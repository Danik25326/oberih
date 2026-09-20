"""
Oberih AST Evaluator v0.1

Бере дерево розбору (Lark Tree) від parser.py і ВИКОНУЄ його напряму -
це і є той компонент, який перетворює наш прототип із "ручного перекладу
прикладів на Python" на справжню мову, що виконує довільний .obh файл.

Обмеження цієї версії (чесно, щоб не створювати ілюзій):
- Немає if/else, циклів, складних типів - тільки let/return/виклики,
  саме того, що використано в наших трьох еталонних прикладах.
- circuitBreaker, idempotent, cache парсяться, але поки що не впливають
  на виконання (Tier 2, буде додано наступним кроком).
- Result<T,E> моделюється через винятки Python, а не окремий тип даних;
  оператор "?" синтаксично присутній, але семантично прозорий (помилки
  і так проштовхуються вгору через виключення).
"""

from lark import Tree, Token
from interpreter import call_resilient, network, _timeout_executor
import bytecode_compiler
import bytecode_vm

_bytecode_cache = {}  # ім'я функції -> байткод, або None якщо непідтримувано


# ---------------------------------------------------------------------------
# Середовище виконання (змінні функції)
# ---------------------------------------------------------------------------

class Environment:
    def __init__(self):
        self.vars = {}

    def set(self, name, value):
        self.vars[name] = value

    def get(self, name):
        return self.vars[name]

    def has(self, name):
        return name in self.vars


class NameRef:
    """Маркер нерозв'язаного ідентифікатора - ім'я функції або зовнішнього
    об'єкта (http, paymentGateway...), яке ще належить викликати."""
    def __init__(self, name):
        self.name = name


class ReturnValue(Exception):
    """Сигнал 'return' - дозволяє вийти з будь-якої глибини вкладених
    if/while блоків прямо до виклику функції, що їх містить."""
    def __init__(self, value):
        self.value = value


# ---------------------------------------------------------------------------
# Вбудовані заглушки (стандартна бібліотека-мінімум для демонстрації)
# ---------------------------------------------------------------------------

def _stub_cached_summary(order_id):
    return {"source": "cache", "order_id": order_id, "degraded": True}


def _stub_cached_user(user_id):
    return {"source": "cache", "id": user_id, "name": "Cached User"}


def _stub_last_known_weather(city):
    return {"source": "cache", "city": city, "condition": "unknown (stale)"}


STUB_FUNCTIONS = {
    "cachedSummary": _stub_cached_summary,
    "cachedUser": _stub_cached_user,
    "lastKnownWeather": _stub_last_known_weather,
}


def _derive_service_key(url):
    """'/orders/123' -> 'http:orders'  - дозволяє мокати кожен ендпоінт окремо."""
    parts = url.strip("/").split("/")
    return f"http:{parts[0]}" if parts and parts[0] else "http:unknown"


def call_builtin_method(obj_name, method_name, args):
    if obj_name == "http" and method_name == "get":
        import interpreter
        if interpreter.HTTP_MODE == "real":
            return interpreter.real_http.get(args[0])
        return network.call(_derive_service_key(args[0]))
    if obj_name == "paymentGateway" and method_name == "charge":
        return network.call("paymentGateway")
    if obj_name == "weatherApi" and method_name == "get":
        return network.call("weatherApi")
    if obj_name == "llm" and method_name == "call":
        from interpreter import llm_service
        return llm_service.call(args[0])
    if obj_name == "agent" and method_name == "call":
        from interpreter import llm_service
        return llm_service.call(args[0])
    raise NameError(f"Невідомий вбудований виклик: {obj_name}.{method_name}()")


# ---------------------------------------------------------------------------
# Реєстр функцій програми
# ---------------------------------------------------------------------------

REGISTRY = {}
STRUCTS = {}  # ім'я структури -> {"type_params": [...], "fields": [(поле, type_expr_node), ...]}
ENUMS = {}    # ім'я enum -> set варіантів (MVP: без асоційованих даних)


def load_program(tree):
    REGISTRY.clear()
    STRUCTS.clear()
    ENUMS.clear()
    for node in tree.children:
        if node.data == "enum_decl":
            name = str(node.children[0])
            variants = {str(v) for v in node.children[1:]}
            ENUMS[name] = variants
            continue

        if node.data == "struct_decl":
            idx = 0
            if isinstance(node.children[idx], Token) and node.children[idx].type == "PRIVATE_KW":
                idx += 1  # приватність структур впливає лише на видимість при import
            name = str(node.children[idx]); idx += 1

            type_params = []
            if idx < len(node.children) and node.children[idx].data == "generic_params":
                type_params = [str(t) for t in node.children[idx].children]
                idx += 1

            fields = []
            for f in node.children[idx:]:
                fields.append((str(f.children[0]), f.children[1]))
            STRUCTS[name] = {"type_params": type_params, "fields": fields}
            continue

        fn_node = node
        idx = 0

        is_private = False
        if isinstance(fn_node.children[idx], Token) and fn_node.children[idx].type == "PRIVATE_KW":
            is_private = True
            idx += 1

        is_resilient = False
        if isinstance(fn_node.children[idx], Token) and fn_node.children[idx].type == "RESILIENT_KW":
            is_resilient = True
            idx += 1

        name = str(fn_node.children[idx]); idx += 1

        if idx < len(fn_node.children) and fn_node.children[idx].data == "method_suffix":
            method_name = str(fn_node.children[idx].children[0])
            name = f"{name}.{method_name}"
            idx += 1

        type_params = []
        if idx < len(fn_node.children) and fn_node.children[idx].data == "generic_params":
            type_params = [str(t) for t in fn_node.children[idx].children]
            idx += 1

        params = []
        param_type_nodes = []
        if idx < len(fn_node.children) and fn_node.children[idx].data == "params":
            for p in fn_node.children[idx].children:
                params.append(str(p.children[0]))
                param_type_nodes.append(p.children[1])
            idx += 1

        return_type_node = None
        if idx < len(fn_node.children) and fn_node.children[idx].data == "type_expr":
            return_type_node = fn_node.children[idx]
            idx += 1

        modifier_nodes = []
        while idx < len(fn_node.children) and fn_node.children[idx].data in (
            "modifier", "bare_modifier"
        ):
            modifier_nodes.append(fn_node.children[idx])
            idx += 1

        block_node = fn_node.children[idx]  # завжди останній child - тіло функції
        REGISTRY[name] = {
            "is_resilient": is_resilient,
            "is_private": is_private,
            "origin_file": getattr(fn_node, "origin_file", None),
            "type_params": type_params,
            "params": params,
            "param_type_nodes": param_type_nodes,
            "return_type_node": return_type_node,
            "modifier_nodes": modifier_nodes,
            "body": block_node.children,
        }
    return REGISTRY


# ---------------------------------------------------------------------------
# Обчислення модифікаторів (deadline, retryBudget, retries, fallback, ...)
# ---------------------------------------------------------------------------

def _eval_value_node(value_node, env):
    child = value_node.children[0]
    if isinstance(child, Tree) and child.data == "duration":
        num_tok, unit_tok = child.children
        num = float(str(num_tok))
        mult = {"ms": 0.001, "s": 1.0, "m": 60.0}[str(unit_tok)]
        return num * mult
    return eval_expr(child, env)


def build_modifiers(modifier_nodes, env):
    mods = {}
    for m in modifier_nodes:
        if m.data == "bare_modifier":
            mods[str(m.children[0])] = True
            continue

        name = str(m.children[0])
        arg_nodes = m.children[1:]
        positional, named = [], {}
        for a in arg_nodes:
            if a.data == "positional_arg":
                if name in ("fallback", "emergencyFallback"):
                    # ЛІНИВЕ обчислення: fallback може посилатись на локальні
                    # змінні функції (напр. orderId) і має виконуватись лише
                    # якщо бюджет справді вичерпано, а не завжди.
                    value_node = a.children[0]
                    positional.append(lambda vn=value_node, e=env: _eval_value_node(vn, e))
                else:
                    positional.append(_eval_value_node(a.children[0], env))
            else:  # named_arg
                key = str(a.children[0])
                named[key] = _eval_value_node(a.children[1], env)

        if name == "deadline":
            mods["deadline"] = positional[0]
        elif name == "retryBudget":
            mods["retryBudget"] = int(positional[0])
        elif name == "retries":
            mods["retries"] = int(positional[0])
        elif name == "fallback":
            thunk = positional[0]
            mods["fallback"] = thunk
        elif name == "emergencyFallback":
            thunk = positional[0]
            mods["emergency_fallback"] = thunk
        elif name == "circuitBreaker":
            mods["circuitBreaker"] = {
                "failThreshold": int(named["failThreshold"]),
                "cooldown": named["cooldown"],
            }
        elif name == "idempotent":
            mods["idempotent"] = {"key": named.get("key", positional[0] if positional else None)}
        elif name == "cache":
            mods["cache"] = {"ttl": named["ttl"]}
        elif name == "timeout":
            mods["timeout"] = positional[0]
        elif name == "rateLimit":
            mods["rateLimit"] = {"n": int(positional[0]), "per": named["per"]}
        elif name == "bulkhead":
            mods["bulkhead"] = {"maxConcurrent": int(named["maxConcurrent"])}
        elif name == "hedging":
            mods["hedging"] = {"after": named["after"]}
        elif name == "budget":
            mods.setdefault("budget", {})
            for k, v in named.items():
                if k != "currency":  # currency - метадані, не рахунок ресурсу
                    mods["budget"][k] = v
        elif name == "saturating":
            mods["saturating"] = {"min": int(named["min"]), "max": int(named["max"])}
        else:
            mods.setdefault("_unknown_modifiers", []).append(name)
    return mods


# ---------------------------------------------------------------------------
# Виконання тіла функції
# ---------------------------------------------------------------------------

def _assign_to_lvalue(node, value, env):
    """Присвоює value за адресою, яку описує node: проста змінна (NAME),
    елемент контейнера (x[i]), поле структури (x.f), чи довільний ланцюжок
    з них (x.a.b, arr[0].field, тощо)."""
    while isinstance(node, Tree) and node.data in ("atom", "try_op"):
        node = node.children[0]

    if isinstance(node, Token) and node.type == "NAME":
        env.set(str(node), value)
        return

    if isinstance(node, Tree) and node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]

        if not postfixes:
            inner = atom_node.children[0]
            if isinstance(inner, Token) and inner.type == "NAME":
                env.set(str(inner), value)
                return
            raise SyntaxError("Неприпустима ліва частина присвоєння")

        if isinstance(atom_node, Tree) and atom_node.data == "atom" and (
            isinstance(atom_node.children[0], Token) and atom_node.children[0].type == "NAME"
        ):
            current = env.get(str(atom_node.children[0]))
        else:
            current = eval_expr(atom_node, env)

        for pf in postfixes[:-1]:
            if pf.data == "field_access":
                current = current[str(pf.children[0])]
            elif pf.data == "index_access":
                idx = eval_expr(pf.children[0], env)
                if isinstance(idx, float) and idx.is_integer():
                    idx = int(idx)
                current = current[idx]
            else:
                raise SyntaxError("Неприпустима ліва частина присвоєння")

        last_pf = postfixes[-1]
        if last_pf.data == "field_access":
            current[str(last_pf.children[0])] = value
        elif last_pf.data == "index_access":
            idx = eval_expr(last_pf.children[0], env)
            if isinstance(idx, float) and idx.is_integer():
                idx = int(idx)
            current[idx] = value
        else:
            raise SyntaxError("Неприпустима ліва частина присвоєння")
        return

    raise SyntaxError("Неприпустима ліва частина присвоєння")


def exec_block(stmts, env):
    for stmt in stmts:
        if stmt.data == "let_stmt":
            env.set(str(stmt.children[0]), eval_expr(stmt.children[1], env))
        elif stmt.data == "expr_or_assign_stmt":
            if len(stmt.children) == 1:
                eval_expr(stmt.children[0], env)  # звичайний вираз-інструкція (напр. print(...))
            else:
                lhs_node, rhs_node = stmt.children
                value = eval_expr(rhs_node, env)
                _assign_to_lvalue(lhs_node, value, env)
        elif stmt.data == "return_stmt":
            raise ReturnValue(eval_expr(stmt.children[0], env))
        elif stmt.data == "if_stmt":
            cond = eval_expr(stmt.children[0], env)
            then_block = stmt.children[1]
            else_block = stmt.children[2] if len(stmt.children) > 2 else None
            if cond:
                exec_block(then_block.children, env)
            elif else_block is not None:
                exec_block(else_block.children, env)
        elif stmt.data == "while_stmt":
            cond_node, body_block = stmt.children
            while eval_expr(cond_node, env):
                exec_block(body_block.children, env)
        elif stmt.data == "for_stmt":
            var_name = str(stmt.children[0])
            iterable = eval_expr(stmt.children[1], env)
            body_block = stmt.children[2]
            if isinstance(iterable, dict):
                iterable = list(iterable.keys())
            for item in iterable:
                env.set(var_name, item)
                exec_block(body_block.children, env)
        else:
            raise NotImplementedError(f"Невідомий тип інструкції: {stmt.data}")


def _contains_agent_call(node):
    """Шукає виклик agent.call(...) де завгодно в тілі функції -
    використовується, щоб автоматично увімкнути durable+traced."""
    if isinstance(node, Token):
        return False
    if not isinstance(node, Tree):
        return False
    if node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]
        if (
            isinstance(atom_node, Tree) and atom_node.data == "atom"
            and isinstance(atom_node.children[0], Token)
            and atom_node.children[0].type == "NAME"
            and str(atom_node.children[0]) == "agent"
        ):
            for pf in postfixes:
                if pf.data == "method_call" and str(pf.children[0]) == "call":
                    return True
    return any(_contains_agent_call(c) for c in node.children)


def call_user_function(name, args):
    fn = REGISTRY[name]

    # Модифікатори (deadline/retryBudget/fallback тощо) обчислюються ОДИН
    # РАЗ на виклик - їм потрібен env з параметрами (fallback може
    # посилатись на них, напр. fallback(cachedSummary(orderId))).
    modifiers_env = Environment()
    for pname, val in zip(fn["params"], args):
        modifiers_env.set(pname, val)

    # Тіло функції пробуємо скомпілювати в байткод НЕЗАЛЕЖНО від того,
    # resilient це чи ні - обгортка стійкості (retry/budget) лишається в
    # Python, а швидкість виконання самого тіла (цикли/арифметика/умови)
    # виграє однаково. Якщо тіло містить http/llm/method_call - компілятор
    # байткоду прозоро відмовляється, і ми падаємо на tree-walking.
    if name not in _bytecode_cache:
        try:
            _bytecode_cache[name] = bytecode_compiler.compile_function_body(fn["body"])
        except bytecode_compiler.UnsupportedNode:
            _bytecode_cache[name] = None
    bytecode = _bytecode_cache[name]

    if bytecode is not None:
        def body_fn(*call_args):
            # Свіжий набір змінних на КОЖНУ спробу - жодного витоку стану
            # між retry-спробами однієї й тієї ж функції.
            variables = dict(zip(fn["params"], call_args))
            return bytecode_vm.run(bytecode, variables)
    else:
        def body_fn(*call_args):
            local_env = Environment()
            for pname, val in zip(fn["params"], call_args):
                local_env.set(pname, val)
            try:
                exec_block(fn["body"], local_env)
                return None  # функція дійшла до кінця без явного return
            except ReturnValue as rv:
                return rv.value

    if not fn["is_resilient"]:
        return body_fn(*args)

    modifiers = build_modifiers(fn["modifier_nodes"], modifiers_env)

    # AI-агент-нативна поведінка: якщо функція викликає agent.call(...),
    # вона автоматично стає durable+traced, навіть якщо це не написано явно.
    if any(_contains_agent_call(stmt) for stmt in fn["body"]):
        modifiers.setdefault("durable", True)
        modifiers.setdefault("traced", True)

    return call_resilient(name, modifiers, body_fn, tuple(args))


def _format_value(v):
    """Людяне форматування значень Oberih для виводу (без Python-репрів)."""
    if isinstance(v, list):
        return "[" + ", ".join(_format_value(x) for x in v) + "]"
    if isinstance(v, dict) and "__result__" in v:
        return f'{v["__result__"]}({_format_value(v["value"])})'
    if isinstance(v, dict) and "__type__" in v and "__fields__" in v:
        fields = ", ".join(_format_value(f) for f in v["__fields__"])
        return f'{v["__type__"]}({fields})'
    if isinstance(v, dict) and "__variant__" in v and "value" not in v:
        return f'{v.get("__enum__", "?")}.{v["__variant__"]}'
    if isinstance(v, dict) and "__type__" in v:
        field_strs = ", ".join(
            f"{k}: {_format_value(val)}" for k, val in v.items() if k != "__type__"
        )
        return f'{v["__type__"]}{{{field_strs}}}'
    if isinstance(v, dict):
        entries = ", ".join(f'"{k}": {_format_value(val)}' for k, val in v.items())
        return "{" + entries + "}"
    if isinstance(v, bool):
        return "true" if v else "false"
    return str(v)


def _builtin_print(*args):
    print(*[_format_value(a) for a in args])
    return None


class SpawnHandle:
    """Результат spawn - обгортка над Future, повертає результат через .join()."""
    def __init__(self, future):
        self.future = future


def call_spawn_handle_method(handle, method_name, args):
    if method_name == "join":
        return handle.future.result()
    if method_name == "isDone":
        return handle.future.done()
    raise NameError(f"Невідомий метод spawn-хендлу: .{method_name}()")


def call_container_method(container, method_name, args):
    if isinstance(container, list):
        if method_name == "push":
            container.append(args[0])
            return container
        if method_name == "len":
            return len(container)
        if method_name == "pop":
            return container.pop()
    if isinstance(container, dict):
        if method_name == "has":
            return args[0] in container
        if method_name == "len":
            return len(container)
        if method_name == "keys":
            return list(container.keys())
    raise NameError(f"Невідомий метод для {type(container).__name__}: .{method_name}()")


def call_by_name(name, args, env):
    if name in REGISTRY:
        return call_user_function(name, args)
    if name == "print":
        return _builtin_print(*args)
    if name == "len":
        return len(args[0])
    if name == "abs":
        return abs(args[0])
    if name == "min":
        return min(args)
    if name == "max":
        return max(args)
    if name == "sqrt":
        return args[0] ** 0.5
    if name == "round":
        return round(args[0]) if len(args) == 1 else round(args[0], int(args[1]))
    if name == "floor":
        import math
        return math.floor(args[0])
    if name == "ceil":
        import math
        return math.ceil(args[0])
    if name == "upper":
        return args[0].upper()
    if name == "lower":
        return args[0].lower()
    if name == "trim":
        return args[0].strip()
    if name == "split":
        return args[0].split(args[1])
    if name == "join":
        return args[1].join(str(x) for x in args[0])
    if name == "contains":
        return args[1] in args[0]
    if name == "replace":
        return args[0].replace(args[1], args[2])
    if name == "toString":
        return _format_value(args[0])
    if name == "toNumber":
        s = args[0]
        return float(s) if "." in s else int(s)
    if name in ("Ok", "Err"):
        return {"__result__": name, "value": args[0] if args else None}
    if name in STUB_FUNCTIONS:
        return STUB_FUNCTIONS[name](*args)
    if name in STRUCTS:
        field_names = [f for f, _ in STRUCTS[name]["fields"]]
        result = {"__type__": name}
        for fname, val in zip(field_names, args):
            result[fname] = val
        return result
    # Невідома функція з великої літери -> генерична структура без означення
    return {"__type__": name, "__fields__": args}


# ---------------------------------------------------------------------------
# Обчислення виразів
# ---------------------------------------------------------------------------

_ESCAPE_MAP = {"\\n": "\n", "\\t": "\t", "\\\"": "\"", "\\\\": "\\", "\\r": "\r"}


def _unescape_string(raw):
    inner = raw[1:-1]
    for esc, real in _ESCAPE_MAP.items():
        inner = inner.replace(esc, real)
    return inner


def _parse_number(raw):
    return float(raw) if "." in raw else int(raw)


def eval_expr(node, env):
    if isinstance(node, Token):
        if node.type == "STRING":
            return _unescape_string(str(node))
        if node.type == "SIGNED_NUMBER":
            return _parse_number(str(node))
        if node.type == "BOOL":
            return str(node) == "true"
        if node.type == "NAME":
            return env.get(str(node)) if env.has(str(node)) else NameRef(str(node))
        raise NotImplementedError(f"Невідомий токен: {node.type}")

    if node.data == "atom":
        return eval_expr(node.children[0], env)

    if node.data == "add":
        left, right = node.children
        l, r = eval_expr(left, env), eval_expr(right, env)
        return l + r  # рядки конкатенуються, числа додаються - той самий "+"

    if node.data == "sub":
        left, right = node.children
        return eval_expr(left, env) - eval_expr(right, env)

    if node.data == "mul":
        left, right = node.children
        return eval_expr(left, env) * eval_expr(right, env)

    if node.data == "div":
        left, right = node.children
        l, r = eval_expr(left, env), eval_expr(right, env)
        if r == 0:
            raise ZeroDivisionError("ділення на нуль в Oberih-програмі")
        return l / r

    if node.data == "list_literal":
        return [eval_expr(c, env) for c in node.children]

    if node.data == "map_literal":
        result = {}
        for entry in node.children:
            key = eval_expr(entry.children[0], env)
            value = eval_expr(entry.children[1], env)
            result[key] = value
        return result

    if node.data == "spawn_expr":
        fn_name = str(node.children[0])
        args_node = node.children[1] if len(node.children) > 1 else None
        arg_nodes = args_node.children if args_node is not None else []
        call_args = [eval_expr(a, env) for a in arg_nodes]
        future = _timeout_executor.submit(call_by_name, fn_name, call_args, None)
        return SpawnHandle(future)

    if node.data == "match_expr":
        scrutinee = eval_expr(node.children[0], env)
        for arm in node.children[1:]:
            pattern_node, arm_expr = arm.children
            if pattern_node.data == "wildcard_pattern":
                return eval_expr(arm_expr, env)
            if pattern_node.data == "literal_pattern":
                literal = eval_expr(pattern_node.children[0], env)
                if scrutinee == literal:
                    return eval_expr(arm_expr, env)
            if pattern_node.data == "ctor_pattern":
                ctor_name = str(pattern_node.children[0])
                bind_name = str(pattern_node.children[1])
                if isinstance(scrutinee, dict) and scrutinee.get("__result__") == ctor_name:
                    env.set(bind_name, scrutinee["value"])
                    return eval_expr(arm_expr, env)
            if pattern_node.data == "variant_pattern":
                variant_name = str(pattern_node.children[0])
                if isinstance(scrutinee, dict) and scrutinee.get("__variant__") == variant_name:
                    return eval_expr(arm_expr, env)
        raise RuntimeError(
            f"Жоден варіант match не підійшов для значення {scrutinee!r}"
        )

    if node.data == "comparison":
        left, op_tok, right = node.children
        l, r = eval_expr(left, env), eval_expr(right, env)
        op = str(op_tok)
        if op == "==":
            return l == r
        if op == "!=":
            return l != r
        if op == "<":
            return l < r
        if op == ">":
            return l > r
        if op == "<=":
            return l <= r
        if op == ">=":
            return l >= r
        raise RuntimeError(f"Невідомий оператор порівняння: {op}")

    if node.data == "try_op":
        # У MVP помилки і так проштовхуються через Python-виключення,
        # тому "?" синтаксично присутній, а семантично прозорий.
        return eval_expr(node.children[0], env)

    if node.data == "primary":
        return eval_primary(node, env)

    raise NotImplementedError(f"Невідомий тип вузла: {node.data}")


def eval_primary(node, env):
    atom_node = node.children[0]
    postfixes = node.children[1:]
    current = eval_expr(atom_node, env)

    for pf in postfixes:
        if pf.data == "call_args":
            args = _eval_args(pf, env)
            if isinstance(current, NameRef):
                current = call_by_name(current.name, args, env)
            else:
                raise TypeError("Значення не є функцією, що викликається")
        elif pf.data == "field_access":
            field = str(pf.children[0])
            if isinstance(current, NameRef) and current.name in ENUMS:
                if field not in ENUMS[current.name]:
                    raise AttributeError(
                        f"Enum '{current.name}' не має варіанту '{field}'"
                    )
                current = {"__enum__": current.name, "__variant__": field}
            elif isinstance(current, dict) and field in current:
                current = current[field]
            else:
                raise AttributeError(f"Немає поля '{field}' у {current!r}")
        elif pf.data == "method_call":
            method = str(pf.children[0])
            args = (
                [eval_expr(c, env) for c in pf.children[1].children]
                if len(pf.children) > 1 else []
            )
            if isinstance(current, NameRef):
                current = call_builtin_method(current.name, method, args)
            elif (
                isinstance(current, dict)
                and current.get("__type__") in STRUCTS
                and f"{current['__type__']}.{method}" in REGISTRY
            ):
                current = call_by_name(f"{current['__type__']}.{method}", [current] + list(args), env)
            elif isinstance(current, (list, dict)):
                current = call_container_method(current, method, args)
            elif isinstance(current, SpawnHandle):
                current = call_spawn_handle_method(current, method, args)
            else:
                raise TypeError(f"Не можна викликати метод '{method}' на {current!r}")
        elif pf.data == "index_access":
            idx = eval_expr(pf.children[0], env)
            if isinstance(idx, float) and idx.is_integer():
                idx = int(idx)
            current = current[idx]
        else:
            raise NotImplementedError(f"Невідомий постфікс: {pf.data}")

    return current


def _eval_args(call_args_node, env):
    if not call_args_node.children:
        return []
    args_node = call_args_node.children[0]
    return [eval_expr(c, env) for c in args_node.children]
