"""
Oberih Bytecode Compiler v0.1 - Фаза 2 (частково)

Компілює AST у прості лінійні байткод-інструкції для "гарячого шляху":
let, assign, return, if/else, while, арифметика, порівняння.

НЕ компілює: виклики функцій, resilient-модифікатори, http/llm/agent
виклики - усе це продовжує йти через evaluator.py (tree-walking), бо
виконується рідко (раз на виклик функції), а не в гарячому циклі, де
швидкість справді критична.

Формат інструкції: (OPCODE, operand) - operand залежить від OPCODE.
"""

from lark import Tree, Token

# Опкоди
PUSH_CONST = "PUSH_CONST"
LOAD_VAR = "LOAD_VAR"
STORE_VAR = "STORE_VAR"
ADD = "ADD"
SUB = "SUB"
MUL = "MUL"
DIV = "DIV"
CMP = "CMP"          # operand: рядок оператора ("<", "<=", ...)
JUMP = "JUMP"
JUMP_IF_FALSE = "JUMP_IF_FALSE"
RETURN = "RETURN"
POP = "POP"
CALL_FUNCTION = "CALL_FUNCTION"            # operand: (fn_name, кількість_аргументів)
TAIL_CALL_FUNCTION = "TAIL_CALL_FUNCTION"  # те саме, але в хвостовій позиції - без росту стеку


def _extract_direct_call(node):
    """Якщо node - це проста форма name(args...) (рівно один call_args
    постфікс на NAME-атомі), повертає (ім'я_функції, [вузли_аргументів]).
    Інакше None."""
    if not (isinstance(node, Tree) and node.data == "primary"):
        return None
    atom_node = node.children[0]
    postfixes = node.children[1:]
    if not (
        len(postfixes) == 1
        and postfixes[0].data == "call_args"
        and isinstance(atom_node, Tree) and atom_node.data == "atom"
        and isinstance(atom_node.children[0], Token)
        and atom_node.children[0].type == "NAME"
    ):
        return None
    fn_name = str(atom_node.children[0])
    call_args_node = postfixes[0]
    arg_nodes = call_args_node.children[0].children if call_args_node.children else []
    return fn_name, arg_nodes


class UnsupportedNode(Exception):
    """Кидається, коли в тілі є конструкція, яку байткод-компілятор ще не
    вміє компілювати (виклик функції, resilient-модифікатор, тощо) -
    у такому разі evaluator.py відкатується на tree-walking для цієї функції."""
    pass


def compile_function_body(stmts):
    """Повертає список байткод-інструкцій або кидає UnsupportedNode."""
    code = []
    _compile_block(stmts, code)
    return code


def _compile_block(stmts, code):
    for stmt in stmts:
        _compile_stmt(stmt, code)


def _bare_name_of(node):
    while isinstance(node, Tree) and node.data in ("atom", "try_op"):
        node = node.children[0]
    if isinstance(node, Token) and node.type == "NAME":
        return str(node)
    return None


def _compile_stmt(stmt, code):
    if stmt.data == "let_stmt":
        name = str(stmt.children[0])
        _compile_expr(stmt.children[1], code)
        code.append((STORE_VAR, name))

    elif stmt.data == "return_stmt":
        expr_node = stmt.children[0]

        # Розгортаємо "?" (try_op) - він прозорий, tail call лишається tail call.
        inner = expr_node
        while isinstance(inner, Tree) and inner.data == "try_op":
            inner = inner.children[0]

        tail_call = _extract_direct_call(inner)
        if tail_call is not None:
            fn_name, arg_nodes = tail_call
            for a in arg_nodes:
                _compile_expr(a, code)
            code.append((TAIL_CALL_FUNCTION, (fn_name, len(arg_nodes))))
        else:
            _compile_expr(expr_node, code)
            code.append((RETURN, None))

    elif stmt.data == "expr_or_assign_stmt":
        if len(stmt.children) == 1:
            _compile_expr(stmt.children[0], code)
            code.append((POP, None))
        else:
            lhs_node, rhs_node = stmt.children
            bare_name = _bare_name_of(lhs_node)
            if bare_name is None:
                # присвоєння полю/елементу - поза межами гарячого шляху,
                # відкочуємось на tree-walking
                raise UnsupportedNode("присвоєння полю/елементу в гарячому шляху")
            _compile_expr(rhs_node, code)
            code.append((STORE_VAR, bare_name))

    elif stmt.data == "if_stmt":
        _compile_expr(stmt.children[0], code)
        jump_if_false_idx = len(code)
        code.append((JUMP_IF_FALSE, None))  # адресу заповнимо пізніше
        _compile_block(stmt.children[1].children, code)
        if len(stmt.children) > 2:
            jump_over_else_idx = len(code)
            code.append((JUMP, None))
            code[jump_if_false_idx] = (JUMP_IF_FALSE, len(code))
            _compile_block(stmt.children[2].children, code)
            code[jump_over_else_idx] = (JUMP, len(code))
        else:
            code[jump_if_false_idx] = (JUMP_IF_FALSE, len(code))

    elif stmt.data == "while_stmt":
        loop_start = len(code)
        _compile_expr(stmt.children[0], code)
        jump_if_false_idx = len(code)
        code.append((JUMP_IF_FALSE, None))
        _compile_block(stmt.children[1].children, code)
        code.append((JUMP, loop_start))
        code[jump_if_false_idx] = (JUMP_IF_FALSE, len(code))

    else:
        raise UnsupportedNode(f"stmt: {stmt.data}")


def _compile_expr(node, code):
    if isinstance(node, Token):
        if node.type == "STRING":
            code.append((PUSH_CONST, node[1:-1]))
        elif node.type == "SIGNED_NUMBER":
            raw = str(node)
            code.append((PUSH_CONST, float(raw) if "." in raw else int(raw)))
        elif node.type == "BOOL":
            code.append((PUSH_CONST, str(node) == "true"))
        elif node.type == "NAME":
            code.append((LOAD_VAR, str(node)))
        else:
            raise UnsupportedNode(f"token: {node.type}")
        return

    if node.data == "atom":
        _compile_expr(node.children[0], code)
        return

    if node.data == "add":
        left, right = node.children
        _compile_expr(left, code)
        _compile_expr(right, code)
        code.append((ADD, None))
        return

    if node.data == "sub":
        left, right = node.children
        _compile_expr(left, code)
        _compile_expr(right, code)
        code.append((SUB, None))
        return

    if node.data == "mul":
        left, right = node.children
        _compile_expr(left, code)
        _compile_expr(right, code)
        code.append((MUL, None))
        return

    if node.data == "div":
        left, right = node.children
        _compile_expr(left, code)
        _compile_expr(right, code)
        code.append((DIV, None))
        return

    if node.data == "comparison":
        left, op_tok, right = node.children
        _compile_expr(left, code)
        _compile_expr(right, code)
        code.append((CMP, str(op_tok)))
        return

    if node.data == "try_op":
        _compile_expr(node.children[0], code)
        return

    if node.data == "primary":
        # Якщо це просто змінна/константа без постфіксів - підтримуємо.
        atom_node = node.children[0]
        postfixes = node.children[1:]
        if not postfixes:
            _compile_expr(atom_node, code)
            return

        # Простий виклик функції: name(args...) - рівно один call_args
        # постфікс, а атом - звичайне ім'я. Усе складніше (method_call,
        # ланцюжки постфіксів, field_access) залишається на tree-walking.
        direct_call = _extract_direct_call(node)
        if direct_call is not None:
            fn_name, arg_nodes = direct_call
            for a in arg_nodes:
                _compile_expr(a, code)
            code.append((CALL_FUNCTION, (fn_name, len(arg_nodes))))
            return

        raise UnsupportedNode("виклик функції/методу в гарячому шляху")

    raise UnsupportedNode(f"expr: {node.data}")
