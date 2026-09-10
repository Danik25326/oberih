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
from interpreter import call_resilient, network, ResilienceExhausted, NetworkFailure


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
        return network.call(_derive_service_key(args[0]))
    if obj_name == "paymentGateway" and method_name == "charge":
        return network.call("paymentGateway")
    if obj_name == "weatherApi" and method_name == "get":
        return network.call("weatherApi")
    raise NameError(f"Невідомий вбудований виклик: {obj_name}.{method_name}()")


# ---------------------------------------------------------------------------
# Реєстр функцій програми
# ---------------------------------------------------------------------------

REGISTRY = {}


def load_program(tree):
    REGISTRY.clear()
    for fn_node in tree.children:
        idx = 0

        is_resilient = False
        if isinstance(fn_node.children[idx], Token) and fn_node.children[idx].type == "RESILIENT_KW":
            is_resilient = True
            idx += 1

        name = str(fn_node.children[idx]); idx += 1

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
        else:
            mods.setdefault("_unknown_modifiers", []).append(name)
    return mods


# ---------------------------------------------------------------------------
# Виконання тіла функції
# ---------------------------------------------------------------------------

def exec_block(stmts, env):
    for stmt in stmts:
        if stmt.data == "let_stmt":
            env.set(str(stmt.children[0]), eval_expr(stmt.children[1], env))
        elif stmt.data == "assign_stmt":
            env.set(str(stmt.children[0]), eval_expr(stmt.children[1], env))
        elif stmt.data == "return_stmt":
            raise ReturnValue(eval_expr(stmt.children[0], env))
        elif stmt.data == "expr_stmt":
            eval_expr(stmt.children[0], env)
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
        else:
            raise NotImplementedError(f"Невідомий тип інструкції: {stmt.data}")


def call_user_function(name, args):
    fn = REGISTRY[name]
    env = Environment()
    for pname, val in zip(fn["params"], args):
        env.set(pname, val)

    def run_body():
        try:
            exec_block(fn["body"], env)
            return None  # функція дійшла до кінця без явного return
        except ReturnValue as rv:
            return rv.value

    if not fn["is_resilient"]:
        # Звичайна функція - виконується напряму, без вимоги deadline/retryBudget.
        return run_body()

    modifiers = build_modifiers(fn["modifier_nodes"], env)
    return call_resilient(name, modifiers, lambda *_: run_body(), tuple(args))


def _format_value(v):
    """Людяне форматування значень Oberih для виводу (без Python-репрів)."""
    if isinstance(v, dict) and "__result__" in v:
        return f'{v["__result__"]}({_format_value(v["value"])})'
    if isinstance(v, dict) and "__type__" in v:
        fields = ", ".join(_format_value(f) for f in v["__fields__"])
        return f'{v["__type__"]}({fields})'
    if isinstance(v, bool):
        return "true" if v else "false"
    return str(v)


def _builtin_print(*args):
    print(*[_format_value(a) for a in args])
    return None


def call_by_name(name, args, env):
    if name in REGISTRY:
        return call_user_function(name, args)
    if name == "print":
        return _builtin_print(*args)
    if name in ("Ok", "Err"):
        return {"__result__": name, "value": args[0] if args else None}
    if name in STUB_FUNCTIONS:
        return STUB_FUNCTIONS[name](*args)
    # Невідома функція з великої літери -> трактуємо як конструктор структури
    # (напр. OrderSummary(order, user, payment)) - без перевірки полів (Tier 2).
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

    if node.data == "comparison":
        left, op_tok, right = node.children
        l, r = eval_expr(left, env), eval_expr(right, env)
        op = str(op_tok)
        return {
            "==": l == r, "!=": l != r,
            "<": l < r, ">": l > r,
            "<=": l <= r, ">=": l >= r,
        }[op]

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
                raise TypeError(f"Значення не є функцією, що викликається")
        elif pf.data == "field_access":
            field = str(pf.children[0])
            if isinstance(current, dict) and field in current:
                current = current[field]
            else:
                raise AttributeError(f"Немає поля '{field}' у {current!r}")
        elif pf.data == "method_call":
            method = str(pf.children[0])
            args = _eval_args(pf.children[1], env) if len(pf.children) > 1 else []
            if isinstance(current, NameRef):
                current = call_builtin_method(current.name, method, args)
            else:
                raise TypeError(f"Не можна викликати метод '{method}' на {current!r}")
        else:
            raise NotImplementedError(f"Невідомий постфікс: {pf.data}")

    return current


def _eval_args(call_args_node, env):
    if not call_args_node.children:
        return []
    args_node = call_args_node.children[0]
    return [eval_expr(c, env) for c in args_node.children]
