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
    resilient-функцій, які тут викликаються (напряму, з-під if/while)."""
    if isinstance(node, Token):
        return
    if not isinstance(node, Tree):
        return

    if node.data == "primary":
        atom_node = node.children[0]
        postfixes = node.children[1:]
        if (
            isinstance(atom_node, Tree)
            and atom_node.data == "atom"
            and isinstance(atom_node.children[0], Token)
            and atom_node.children[0].type == "NAME"
            and postfixes
            and postfixes[0].data == "call_args"
        ):
            called_name = str(atom_node.children[0])
            if called_name in registry and registry[called_name]["is_resilient"]:
                found.add(called_name)

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


def _base_type_name(type_expr_node):
    if type_expr_node is None:
        return "Dynamic"
    name = str(type_expr_node.children[0])
    return name if name in PRIMITIVES else "Dynamic"


class TypeEnv:
    def __init__(self):
        self.vars = {}

    def set(self, name, t):
        self.vars[name] = t

    def get(self, name):
        return self.vars.get(name, "Dynamic")

    def has(self, name):
        return name in self.vars


def _resolve_signatures(registry):
    for fn in registry.values():
        fn["_param_types"] = [_base_type_name(t) for t in fn["param_type_nodes"]]
        fn["_return_type"] = _base_type_name(fn["return_type_node"])


def infer_expr_type(node, tenv, registry, errors, fn_name):
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
        return infer_expr_type(node.children[0], tenv, registry, errors, fn_name)

    if node.data == "add":
        left, right = node.children
        lt = infer_expr_type(left, tenv, registry, errors, fn_name)
        rt = infer_expr_type(right, tenv, registry, errors, fn_name)
        if "Boolean" in (lt, rt):
            errors.append(f"'{fn_name}': оператор '+' не застосовується до Boolean")
        elif lt != "Dynamic" and rt != "Dynamic" and lt != rt:
            errors.append(
                f"'{fn_name}': неможливо застосувати '+' до {lt} і {rt} "
                f"(рядки конкатенуються з рядками, числа додаються до чисел)"
            )
        return lt if lt != "Dynamic" else rt

    if node.data == "comparison":
        left, _op, right = node.children
        lt = infer_expr_type(left, tenv, registry, errors, fn_name)
        rt = infer_expr_type(right, tenv, registry, errors, fn_name)
        if lt != "Dynamic" and rt != "Dynamic" and lt != rt:
            errors.append(f"'{fn_name}': порівняння {lt} з {rt} - типи несумісні")
        return "Boolean"

    if node.data == "try_op":
        return infer_expr_type(node.children[0], tenv, registry, errors, fn_name)

    if node.data == "primary":
        return _infer_primary_type(node, tenv, registry, errors, fn_name)

    return "Dynamic"


def _infer_primary_type(node, tenv, registry, errors, fn_name):
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
        current_type = infer_expr_type(inner, tenv, registry, errors, fn_name)

    for pf in postfixes:
        if pf.data == "call_args":
            arg_nodes = pf.children[0].children if pf.children else []
            arg_types = [infer_expr_type(a, tenv, registry, errors, fn_name) for a in arg_nodes]

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
                        if exp != "Dynamic" and act != "Dynamic" and exp != act:
                            errors.append(
                                f"'{fn_name}': виклик '{pending_call_name}', "
                                f"аргумент {i} - очікувався {exp}, передано {act}"
                            )
                current_type = target["_return_type"]
            else:
                current_type = "Dynamic"  # stub/Ok/Err/struct-конструктор - не перевіряємо
            pending_call_name = None

        elif pf.data == "field_access":
            current_type = "Dynamic"  # без оголошень структур не можемо знати тип поля

        elif pf.data == "method_call":
            if len(pf.children) > 1:
                for a in pf.children[1].children:
                    infer_expr_type(a, tenv, registry, errors, fn_name)
            current_type = "Dynamic"  # вбудовані http/paymentGateway/... - завжди Dynamic
            pending_call_name = None

    return current_type


def _check_block(stmts, tenv, registry, errors, fn_name, expected_return):
    for stmt in stmts:
        if stmt.data == "let_stmt":
            t = infer_expr_type(stmt.children[1], tenv, registry, errors, fn_name)
            tenv.set(str(stmt.children[0]), t)

        elif stmt.data == "assign_stmt":
            varname = str(stmt.children[0])
            t = infer_expr_type(stmt.children[1], tenv, registry, errors, fn_name)
            if tenv.has(varname):
                old = tenv.get(varname)
                if old != "Dynamic" and t != "Dynamic" and old != t:
                    errors.append(
                        f"'{fn_name}': змінній '{varname}' (тип {old}) присвоюється "
                        f"значення типу {t}"
                    )
            tenv.set(varname, t)

        elif stmt.data == "return_stmt":
            t = infer_expr_type(stmt.children[0], tenv, registry, errors, fn_name)
            if expected_return != "Dynamic" and t != "Dynamic" and t != expected_return:
                errors.append(
                    f"'{fn_name}': return повертає {t}, а сигнатура функції "
                    f"оголошує {expected_return}"
                )

        elif stmt.data == "expr_stmt":
            infer_expr_type(stmt.children[0], tenv, registry, errors, fn_name)

        elif stmt.data == "if_stmt":
            cond_t = infer_expr_type(stmt.children[0], tenv, registry, errors, fn_name)
            if cond_t not in ("Boolean", "Dynamic"):
                errors.append(f"'{fn_name}': умова if має бути Boolean, отримано {cond_t}")
            _check_block(stmt.children[1].children, tenv, registry, errors, fn_name, expected_return)
            if len(stmt.children) > 2:
                _check_block(stmt.children[2].children, tenv, registry, errors, fn_name, expected_return)

        elif stmt.data == "while_stmt":
            cond_t = infer_expr_type(stmt.children[0], tenv, registry, errors, fn_name)
            if cond_t not in ("Boolean", "Dynamic"):
                errors.append(f"'{fn_name}': умова while має бути Boolean, отримано {cond_t}")
            _check_block(stmt.children[1].children, tenv, registry, errors, fn_name, expected_return)


def type_check_program(registry):
    """Повертає список помилок типів (порожній список = все ок)."""
    _resolve_signatures(registry)
    errors = []
    for name, fn in registry.items():
        tenv = TypeEnv()
        for pname, ptype in zip(fn["params"], fn["_param_types"]):
            tenv.set(pname, ptype)
        _check_block(fn["body"], tenv, registry, errors, name, fn["_return_type"])
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
