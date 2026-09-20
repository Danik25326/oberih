"""
Oberih Typechecker v0.1 (мінімальний, але реальний)

Головна перевірка, заради якої ми взагалі проектували мову:
кожна "коренева" resilient-функція (та, яку НЕ викликає жодна інша
resilient-функція - тобто вона є точкою входу в ланцюжок стійкості)
ПОВИННА явно оголосити deadline і retryBudget. Без цього компіляція
не проходить - неможливо випадково забути захист від retry storm.

Вкладені resilient-функції (викликані лише зсередини інших resilient-функцій)
успадковують бюджет від батька і не зобов'язані оголошувати власний.

Чесне обмеження цієї версії: якщо функція викликається і як коренева
(напряму з main), і як вкладена (з іншої resilient fn) - статичний аналіз
класифікує її як "не кореневу" (бо вона комусь вкладена) і не вимагатиме
deadline, що може призвести до помилки виконання в тому шляху виклику,
де вона все ж є кореневою. Повне рішення потребує flow-sensitive аналізу
викликів для кожного шляху окремо - заплановано на майбутню ітерацію.
"""

from lark import Tree, Token


class CompileError(Exception):
    def __init__(self, errors):
        self.errors = errors
        super().__init__("\n".join(errors))


def _modifier_names(modifier_nodes):
    names = set()
    for m in modifier_nodes:
        names.add(str(m.children[0]))
    return names


def _walk_calls(node, registry, found):
    """Рекурсивно обходить AST тіла функції і збирає імена всіх
    resilient-функцій, які тут викликаються (напряму, з-під if/while,
    або як метод obj.method())."""
    if isinstance(node, Token):
        return
    if not isinstance(node, Tree):
        return

    if node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]
        if postfixes and postfixes[0].data == "call_args":
            if (
                isinstance(atom_node, Tree)
                and atom_node.data == "atom"
                and isinstance(atom_node.children[0], Token)
                and atom_node.children[0].type == "NAME"
            ):
                called_name = str(atom_node.children[0])
                if called_name in registry and registry[called_name]["is_resilient"]:
                    found.add(called_name)
        elif postfixes and postfixes[0].data == "method_call":
            method_name = str(postfixes[0].children[0])
            for candidate, fn in registry.items():
                if candidate.endswith(f".{method_name}") and fn["is_resilient"]:
                    found.add(candidate)

    for child in node.children:
        _walk_calls(child, registry, found)


def check_program(registry):
    """Повертає список помилок (порожній список = все ок)."""
    called_by_resilient = set()

    for name, fn in registry.items():
        if not fn["is_resilient"]:
            continue
        for stmt in fn["body"]:
            _walk_calls(stmt, registry, called_by_resilient)

    errors = []
    for name, fn in registry.items():
        if not fn["is_resilient"]:
            continue
        if name in called_by_resilient:
            continue  # вкладена функція - успадковує бюджет, ОК без власного

        mod_names = _modifier_names(fn["modifier_nodes"])
        missing = [m for m in ("deadline", "retryBudget") if m not in mod_names]
        if missing:
            errors.append(
                f"'{name}' - коренева resilient-функція без {', '.join(missing)}. "
                f"Компілятор Oberih вимагає явний deadline і retryBudget на "
                f"кожній кореневій resilient fn, щоб унеможливити випадковий "
                f"retry storm."
            )

    return errors


# ---------------------------------------------------------------------------
# Перевірка типів (градуальна типізація)
#
# Реально перевіряються тільки примітиви: Number, String, Boolean. Будь-який
# інший тип (Result<...>, Order, Money, ...) трактується як Dynamic - ми не
# маємо синтаксису оголошення структур, тож не можемо перевірити їхні поля.
# Це свідомий компроміс градуальної типізації: там, де ми не впевнені - не
# блокуємо компіляцію хибними спрацюваннями. Повна структурна типізація -
# наступна ітерація.
# ---------------------------------------------------------------------------

PRIMITIVES = {"Number", "String", "Boolean"}


def _base_type_name(type_expr_node, structs=None, type_param_scope=None):
    if type_expr_node is None:
        return "Dynamic"
    children = type_expr_node.children
    name = str(children[0])
    generic_args = children[1:]

    if type_param_scope and name in type_param_scope:
        return ("TypeParam", name)

    if name == "List" and len(generic_args) == 1:
        return ("List", _base_type_name(generic_args[0], structs, type_param_scope))
    if name == "Map" and len(generic_args) == 2:
        return (
            "Map",
            _base_type_name(generic_args[0], structs, type_param_scope),
            _base_type_name(generic_args[1], structs, type_param_scope),
        )

    if name in PRIMITIVES:
        return name

    if structs and name in structs:
        struct_type_params = structs[name].get("type_params", [])
        if struct_type_params and len(generic_args) == len(struct_type_params):
            bound_args = tuple(
                _base_type_name(a, structs, type_param_scope) for a in generic_args
            )
            return ("Generic", name, bound_args)
        return name  # неgeneric структура, або generic без явних аргументів

    return "Dynamic"


def _resolve_struct_field_types(structs):
    for name, info in structs.items():
        type_params = info.get("type_params", [])
        info["_field_types"] = [
            (fname, _base_type_name(ftype_node, structs, type_params))
            for fname, ftype_node in info["fields"]
        ]


def _type_name(t):
    """Форматує внутрішнє представлення типу для повідомлень про помилки."""
    if isinstance(t, tuple):
        if t[0] == "List":
            return f"List<{_type_name(t[1])}>"
        if t[0] == "Map":
            return f"Map<{_type_name(t[1])}, {_type_name(t[2])}>"
        if t[0] == "Generic":
            args = ", ".join(_type_name(a) for a in t[2])
            return f"{t[1]}<{args}>"
        if t[0] == "TypeParam":
            return t[1]
        return t[0]
    return t


class TypeEnv:
    def __init__(self):
        self.vars = {}

    def set(self, name, t):
        self.vars[name] = t

    def get(self, name):
        return self.vars.get(name, "Dynamic")

    def has(self, name):
        return name in self.vars


def _resolve_signatures(registry, structs):
    _resolve_struct_field_types(structs)
    for fn in registry.values():
        tps = fn.get("type_params", [])
        fn["_param_types"] = [_base_type_name(t, structs, tps) for t in fn["param_type_nodes"]]
        fn["_return_type"] = _base_type_name(fn["return_type_node"], structs, tps)


def infer_expr_type(node, tenv, registry, structs, errors, fn_name):
    if isinstance(node, Token):
        if node.type == "STRING":
            return "String"
        if node.type == "SIGNED_NUMBER":
            return "Number"
        if node.type == "BOOL":
            return "Boolean"
        if node.type == "NAME":
            return tenv.get(str(node))
        return "Dynamic"

    if node.data == "atom":
        return infer_expr_type(node.children[0], tenv, registry, structs, errors, fn_name)

    if node.data == "add":
        left, right = node.children
        lt = infer_expr_type(left, tenv, registry, structs, errors, fn_name)
        rt = infer_expr_type(right, tenv, registry, structs, errors, fn_name)
        if "Boolean" in (lt, rt):
            errors.append(f"'{fn_name}': оператор '+' не застосовується до Boolean")
        elif lt != "Dynamic" and rt != "Dynamic" and lt != rt:
            errors.append(
                f"'{fn_name}': неможливо застосувати '+' до {_type_name(lt)} і {_type_name(rt)} "
                f"(рядки конкатенуються з рядками, числа додаються до чисел)"
            )
        return lt if lt != "Dynamic" else rt

    if node.data in ("sub", "mul", "div"):
        op_symbol = {"sub": "-", "mul": "*", "div": "/"}[node.data]
        left, right = node.children
        lt = infer_expr_type(left, tenv, registry, structs, errors, fn_name)
        rt = infer_expr_type(right, tenv, registry, structs, errors, fn_name)
        if lt not in ("Number", "Dynamic"):
            errors.append(f"'{fn_name}': лівий операнд '{op_symbol}' має бути Number, отримано {_type_name(lt)}")
        if rt not in ("Number", "Dynamic"):
            errors.append(f"'{fn_name}': правий операнд '{op_symbol}' має бути Number, отримано {_type_name(rt)}")
        return "Number"

    if node.data == "comparison":
        left, _op, right = node.children
        lt = infer_expr_type(left, tenv, registry, structs, errors, fn_name)
        rt = infer_expr_type(right, tenv, registry, structs, errors, fn_name)
        if lt != "Dynamic" and rt != "Dynamic" and lt != rt:
            errors.append(f"'{fn_name}': порівняння {_type_name(lt)} з {_type_name(rt)} - типи несумісні")
        return "Boolean"

    if node.data == "try_op":
        return infer_expr_type(node.children[0], tenv, registry, structs, errors, fn_name)

    if node.data == "list_literal":
        elem_types = [
            infer_expr_type(c, tenv, registry, structs, errors, fn_name) for c in node.children
        ]
        if not elem_types:
            return ("List", "Dynamic")
        first = elem_types[0]
        if all(t == first or t == "Dynamic" or first == "Dynamic" for t in elem_types):
            return ("List", first if first != "Dynamic" else "Dynamic")
        errors.append(
            f"'{fn_name}': елементи масиву мають різні типи: "
            f"{sorted({_type_name(t) for t in elem_types})}"
        )
        return ("List", "Dynamic")

    if node.data == "map_literal":
        key_types, val_types = [], []
        for entry in node.children:
            key_types.append(infer_expr_type(entry.children[0], tenv, registry, structs, errors, fn_name))
            val_types.append(infer_expr_type(entry.children[1], tenv, registry, structs, errors, fn_name))
        if not key_types:
            return ("Map", "Dynamic", "Dynamic")
        kt = key_types[0] if all(t == key_types[0] for t in key_types) else "Dynamic"
        vt = val_types[0] if all(t == val_types[0] for t in val_types) else "Dynamic"
        return ("Map", kt, vt)

    if node.data == "primary":
        return _infer_primary_type(node, tenv, registry, structs, errors, fn_name)

    return "Dynamic"


def _infer_primary_type(node, tenv, registry, structs, errors, fn_name):
    atom_node = node.children[0]
    postfixes = node.children[1:]

    current_type = "Dynamic"
    pending_call_name = None  # ім'я функції, що очікує "(" args ")"

    inner = atom_node.children[0]
    if isinstance(inner, Token) and inner.type == "NAME":
        name_str = str(inner)
        if tenv.has(name_str):
            current_type = tenv.get(name_str)
        else:
            pending_call_name = name_str  # можливо функція/об'єкт - розберемось у постфіксах
    else:
        current_type = infer_expr_type(inner, tenv, registry, structs, errors, fn_name)

    for pf in postfixes:
        if pf.data == "call_args":
            arg_nodes = pf.children[0].children if pf.children else []
            arg_types = [infer_expr_type(a, tenv, registry, structs, errors, fn_name) for a in arg_nodes]

            if pending_call_name is not None and pending_call_name in registry:
                target = registry[pending_call_name]
                expected = target["_param_types"]
                if len(expected) != len(arg_types):
                    errors.append(
                        f"'{fn_name}': виклик '{pending_call_name}' очікує "
                        f"{len(expected)} аргумент(ів), передано {len(arg_types)}"
                    )
                else:
                    for i, (exp, act) in enumerate(zip(expected, arg_types), start=1):
                        if isinstance(exp, tuple) and exp[0] == "TypeParam":
                            continue  # generic-параметр - сумісний з будь-яким типом (без повної інференції)
                        if exp != "Dynamic" and act != "Dynamic" and exp != act:
                            errors.append(
                                f"'{fn_name}': виклик '{pending_call_name}', "
                                f"аргумент {i} - очікувався {_type_name(exp)}, передано {_type_name(act)}"
                            )
                ret = target["_return_type"]
                # Якщо повертається сам generic-параметр (fn identity<T>(x:T)->T),
                # підставляємо тип, виведений з фактичного аргументу на тій самій позиції.
                if isinstance(ret, tuple) and ret[0] == "TypeParam":
                    tp_name = ret[1]
                    substituted = "Dynamic"
                    for exp, act in zip(expected, arg_types):
                        if isinstance(exp, tuple) and exp[0] == "TypeParam" and exp[1] == tp_name:
                            substituted = act
                            break
                    current_type = substituted
                else:
                    current_type = ret
            elif pending_call_name is not None and pending_call_name in structs:
                struct_info = structs[pending_call_name]
                field_defs = struct_info["_field_types"]
                type_params = struct_info.get("type_params", [])
                if len(field_defs) != len(arg_types):
                    errors.append(
                        f"'{fn_name}': конструктор '{pending_call_name}' очікує "
                        f"{len(field_defs)} поле(ів), передано {len(arg_types)}"
                    )
                    current_type = pending_call_name
                else:
                    inferred = {}
                    for i, ((fname, exp), act) in enumerate(zip(field_defs, arg_types), start=1):
                        if isinstance(exp, tuple) and exp[0] == "TypeParam":
                            tp_name = exp[1]
                            prior = inferred.get(tp_name)
                            if prior and prior != "Dynamic" and act != "Dynamic" and prior != act:
                                errors.append(
                                    f"'{fn_name}': конструктор '{pending_call_name}', "
                                    f"тип-параметр '{tp_name}' отримав різні типи: "
                                    f"{_type_name(prior)} і {_type_name(act)}"
                                )
                            inferred.setdefault(tp_name, act)
                        elif exp != "Dynamic" and act != "Dynamic" and exp != act:
                            errors.append(
                                f"'{fn_name}': конструктор '{pending_call_name}', "
                                f"поле '{fname}' - очікувався {_type_name(exp)}, передано {_type_name(act)}"
                            )
                    if type_params:
                        bound_args = tuple(inferred.get(tp, "Dynamic") for tp in type_params)
                        current_type = ("Generic", pending_call_name, bound_args)
                    else:
                        current_type = pending_call_name
            else:
                current_type = "Dynamic"  # stub/Ok/Err/невідома структура - не перевіряємо
            pending_call_name = None

        elif pf.data == "field_access":
            field = str(pf.children[0])
            struct_name = None
            substitution = {}
            if isinstance(current_type, tuple) and current_type[0] == "Generic":
                struct_name = current_type[1]
                type_params = structs.get(struct_name, {}).get("type_params", [])
                substitution = dict(zip(type_params, current_type[2]))
            elif current_type in structs:
                struct_name = current_type

            if struct_name is not None:
                field_types = dict(structs[struct_name]["_field_types"])
                if field not in field_types:
                    errors.append(
                        f"'{fn_name}': структура '{struct_name}' не має поля '{field}'"
                    )
                    current_type = "Dynamic"
                else:
                    ft = field_types[field]
                    if isinstance(ft, tuple) and ft[0] == "TypeParam":
                        current_type = substitution.get(ft[1], "Dynamic")
                    else:
                        current_type = ft
            else:
                current_type = "Dynamic"  # невідома структура - не перевіряємо

        elif pf.data == "index_access":
            idx_type = infer_expr_type(pf.children[0], tenv, registry, structs, errors, fn_name)
            if isinstance(current_type, tuple) and current_type[0] == "List":
                if idx_type not in ("Number", "Dynamic"):
                    errors.append(
                        f"'{fn_name}': індекс масиву має бути Number, отримано {_type_name(idx_type)}"
                    )
                current_type = current_type[1]
            elif isinstance(current_type, tuple) and current_type[0] == "Map":
                key_t = current_type[1]
                if key_t != "Dynamic" and idx_type != "Dynamic" and key_t != idx_type:
                    errors.append(
                        f"'{fn_name}': ключ словника має бути {_type_name(key_t)}, "
                        f"отримано {_type_name(idx_type)}"
                    )
                current_type = current_type[2]
            else:
                current_type = "Dynamic"

        elif pf.data == "method_call":
            method_name = str(pf.children[0])
            arg_nodes = pf.children[1].children if len(pf.children) > 1 else []
            arg_types = [
                infer_expr_type(a, tenv, registry, structs, errors, fn_name) for a in arg_nodes
            ]

            receiver_struct = None
            if isinstance(current_type, tuple) and current_type[0] == "Generic":
                receiver_struct = current_type[1]
            elif current_type in structs:
                receiver_struct = current_type

            method_key = f"{receiver_struct}.{method_name}" if receiver_struct else None
            if method_key and method_key in registry:
                target = registry[method_key]
                expected = target["_param_types"][1:]  # пропускаємо self
                if len(expected) != len(arg_types):
                    errors.append(
                        f"'{fn_name}': виклик '.{method_name}()' очікує "
                        f"{len(expected)} аргумент(ів), передано {len(arg_types)}"
                    )
                else:
                    for i, (exp, act) in enumerate(zip(expected, arg_types), start=1):
                        if isinstance(exp, tuple) and exp[0] == "TypeParam":
                            continue
                        if exp != "Dynamic" and act != "Dynamic" and exp != act:
                            errors.append(
                                f"'{fn_name}': виклик '.{method_name}()', аргумент {i} - "
                                f"очікувався {_type_name(exp)}, передано {_type_name(act)}"
                            )
                current_type = target["_return_type"]
            else:
                current_type = "Dynamic"  # вбудовані http/paymentGateway/... чи невідомий метод
            pending_call_name = None

    return current_type


def _bare_name_of(node):
    """Якщо node - це просто змінна (без постфіксів), повертає її ім'я."""
    while isinstance(node, Tree) and node.data in ("atom", "try_op"):
        node = node.children[0]
    if isinstance(node, Token) and node.type == "NAME":
        return str(node)
    return None


def _check_block(stmts, tenv, registry, structs, errors, fn_name, expected_return):
    for stmt in stmts:
        if stmt.data == "let_stmt":
            t = infer_expr_type(stmt.children[1], tenv, registry, structs, errors, fn_name)
            tenv.set(str(stmt.children[0]), t)

        elif stmt.data == "expr_or_assign_stmt":
            if len(stmt.children) == 1:
                infer_expr_type(stmt.children[0], tenv, registry, structs, errors, fn_name)
            else:
                lhs_node, rhs_node = stmt.children
                rhs_type = infer_expr_type(rhs_node, tenv, registry, structs, errors, fn_name)
                bare_name = _bare_name_of(lhs_node)
                if bare_name is not None:
                    if tenv.has(bare_name):
                        old = tenv.get(bare_name)
                        if old != "Dynamic" and rhs_type != "Dynamic" and old != rhs_type:
                            errors.append(
                                f"'{fn_name}': змінній '{bare_name}' (тип {_type_name(old)}) "
                                f"присвоюється значення типу {_type_name(rhs_type)}"
                            )
                    tenv.set(bare_name, rhs_type)
                else:
                    lhs_type = infer_expr_type(lhs_node, tenv, registry, structs, errors, fn_name)
                    if lhs_type != "Dynamic" and rhs_type != "Dynamic" and lhs_type != rhs_type:
                        errors.append(
                            f"'{fn_name}': присвоєння - очікувався {_type_name(lhs_type)}, "
                            f"передано {_type_name(rhs_type)}"
                        )

        elif stmt.data == "return_stmt":
            t = infer_expr_type(stmt.children[0], tenv, registry, structs, errors, fn_name)
            if expected_return != "Dynamic" and t != "Dynamic" and t != expected_return:
                errors.append(
                    f"'{fn_name}': return повертає {_type_name(t)}, а сигнатура функції "
                    f"оголошує {_type_name(expected_return)}"
                )

        elif stmt.data == "if_stmt":
            cond_t = infer_expr_type(stmt.children[0], tenv, registry, structs, errors, fn_name)
            if cond_t not in ("Boolean", "Dynamic"):
                errors.append(f"'{fn_name}': умова if має бути Boolean, отримано {cond_t}")
            _check_block(stmt.children[1].children, tenv, registry, structs, errors, fn_name, expected_return)
            if len(stmt.children) > 2:
                _check_block(stmt.children[2].children, tenv, registry, structs, errors, fn_name, expected_return)

        elif stmt.data == "while_stmt":
            cond_t = infer_expr_type(stmt.children[0], tenv, registry, structs, errors, fn_name)
            if cond_t not in ("Boolean", "Dynamic"):
                errors.append(f"'{fn_name}': умова while має бути Boolean, отримано {cond_t}")
            _check_block(stmt.children[1].children, tenv, registry, structs, errors, fn_name, expected_return)

        elif stmt.data == "for_stmt":
            var_name = str(stmt.children[0])
            infer_expr_type(stmt.children[1], tenv, registry, structs, errors, fn_name)
            # Тип елемента поки не відомий без повної типізації List<T> - Dynamic.
            tenv.set(var_name, "Dynamic")
            _check_block(stmt.children[2].children, tenv, registry, structs, errors, fn_name, expected_return)


def type_check_program(registry, structs):
    """Повертає список помилок типів (порожній список = все ок)."""
    _resolve_signatures(registry, structs)
    errors = []
    for name, fn in registry.items():
        tenv = TypeEnv()
        for pname, ptype in zip(fn["params"], fn["_param_types"]):
            tenv.set(pname, ptype)
        _check_block(fn["body"], tenv, registry, structs, errors, name, fn["_return_type"])
    return errors


# ---------------------------------------------------------------------------
# emergencyFallback не повинен містити I/O - це останній, гарантований рубіж
# ---------------------------------------------------------------------------

def _find_modifier_value_node(modifier_nodes, mod_name):
    for m in modifier_nodes:
        if m.data != "modifier" or str(m.children[0]) != mod_name:
            continue
        for a in m.children[1:]:
            if a.data == "positional_arg":
                return a.children[0]
            elif a.data == "named_arg":
                return a.children[1]
    return None


def _contains_io(node, registry):
    if isinstance(node, Token):
        return False
    if not isinstance(node, Tree):
        return False

    if node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]
        for pf in postfixes:
            if pf.data == "method_call":
                return True  # виклик http/paymentGateway/... - завжди I/O
            if pf.data == "call_args":
                if (
                    isinstance(atom_node, Tree)
                    and atom_node.data == "atom"
                    and isinstance(atom_node.children[0], Token)
                    and atom_node.children[0].type == "NAME"
                ):
                    called = str(atom_node.children[0])
                    if called in registry and registry[called]["is_resilient"]:
                        return True  # виклик іншої resilient fn теж може впасти

    return any(_contains_io(c, registry) for c in node.children)


def check_emergency_fallback_no_io(registry):
    errors = []
    for name, fn in registry.items():
        if not fn["is_resilient"]:
            continue
        value_node = _find_modifier_value_node(fn["modifier_nodes"], "emergencyFallback")
        if value_node is not None and _contains_io(value_node, registry):
            errors.append(
                f"'{name}': emergencyFallback не може містити мережеві виклики "
                f"(I/O) чи виклики resilient-функцій - лише статичні значення. "
                f"Це має бути гарантований рубіж, який сам не може впасти."
            )
    return errors


# ---------------------------------------------------------------------------
# agent.call() без budget(...) - заборонено. Саме ця недбалість спричиняє
# реальні інциденти "AI-агент витратив тисячі доларів у retry-циклі".
# ---------------------------------------------------------------------------

def _contains_agent_call(node):
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


def check_agent_budget_required(registry):
    errors = []
    for name, fn in registry.items():
        if not fn["is_resilient"]:
            continue
        if any(_contains_agent_call(stmt) for stmt in fn["body"]):
            mod_names = _modifier_names(fn["modifier_nodes"])
            if "budget" not in mod_names:
                errors.append(
                    f"'{name}': використовує agent.call(...), але не має "
                    f"budget(...) - компілятор Oberih вимагає явний ліміт "
                    f"токенів/вартості на кожній функції, що звертається до AI-"
                    f"агента, щоб унеможливити неконтрольовані витрати."
                )
    return errors


# ---------------------------------------------------------------------------
# Видимість private - хто кого викликає, а не "чи існує в реєстрі"
#
# Приватна функція лишається в REGISTRY (щоб публічна функція з ТОГО Ж
# файлу могла нею користуватись), але компілятор забороняє виклик private-
# функції з коду, оголошеного в ІНШОМУ файлі. Це компіляторна перевірка,
# а не приховування символу - чесний, реалістичний обсяг "інкапсуляції"
# для мови з єдиним плоским простором імен.
# ---------------------------------------------------------------------------

def _walk_all_calls(node, registry, found):
    """Як _walk_calls, але збирає виклики БУДЬ-ЯКИХ функцій з реєстру,
    не лише resilient - потрібно для перевірки private, яка стосується
    всіх функцій однаково.

    Розпізнає і прості виклики name(args), і виклики методів obj.method(args).
    Для методів немає повного виведення типу отримувача (obj) - тому
    консервативно позначає ВСІ зареєстровані "*.method" з таким іменем як
    можливі цілі. Це може зрідка дати хибне спрацювання при збігу імен
    методів різних структур, але краще так, ніж пропустити реальний обхід
    private - контроль видимості має схилятись до обережності."""
    if isinstance(node, Token):
        return
    if not isinstance(node, Tree):
        return

    if node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]
        if postfixes and postfixes[0].data == "call_args":
            if (
                isinstance(atom_node, Tree)
                and atom_node.data == "atom"
                and isinstance(atom_node.children[0], Token)
                and atom_node.children[0].type == "NAME"
            ):
                called_name = str(atom_node.children[0])
                if called_name in registry:
                    found.add(called_name)
        elif postfixes and postfixes[0].data == "method_call":
            method_name = str(postfixes[0].children[0])
            for candidate in registry:
                if candidate.endswith(f".{method_name}"):
                    found.add(candidate)

    for child in node.children:
        _walk_all_calls(child, registry, found)


def check_private_visibility(registry):
    errors = []
    for caller_name, caller_fn in registry.items():
        called = set()
        for stmt in caller_fn["body"]:
            _walk_all_calls(stmt, registry, called)

        for callee_name in called:
            callee_fn = registry[callee_name]
            if not callee_fn.get("is_private"):
                continue
            if callee_fn.get("origin_file") != caller_fn.get("origin_file"):
                errors.append(
                    f"'{caller_name}' не може викликати '{callee_name}' - "
                    f"вона оголошена як private в іншому файлі "
                    f"({callee_fn.get('origin_file') or '?'})."
                )
    return errors
