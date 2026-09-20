"""
Oberih Bytecode VM v0.1 - проста стекова машина.

Виконує лінійний список байткод-інструкцій, згенерованих bytecode_compiler.py.
Використовує звичайний Python-список як стек значень і dict як змінні.
"""

from bytecode_compiler import (
    PUSH_CONST, LOAD_VAR, STORE_VAR, ADD, SUB, MUL, DIV, CMP,
    JUMP, JUMP_IF_FALSE, RETURN, POP, CALL_FUNCTION, TAIL_CALL_FUNCTION,
)


class ReturnSignal(Exception):
    def __init__(self, value):
        self.value = value


def run(code, variables):
    """variables: dict з початковими значеннями (параметри функції).
    Повертає значення з RETURN, або None, якщо код закінчився без return.

    Хвостові виклики (TAIL_CALL_FUNCTION) НЕ рекурсують у Python - замість
    цього зовнішній цикл просто перезапускається з новим байткодом/змінними.
    Це справжнє трамплінування: глибина рекурсії Oberih більше не обмежена
    стеком Python."""
    while True:
        stack = []
        pc = 0
        n = len(code)
        jumped_to_new_frame = False

        while pc < n:
            op, arg = code[pc]

            if op == PUSH_CONST:
                stack.append(arg)
                pc += 1

            elif op == LOAD_VAR:
                stack.append(variables[arg])
                pc += 1

            elif op == STORE_VAR:
                variables[arg] = stack.pop()
                pc += 1

            elif op == ADD:
                b = stack.pop()
                a = stack.pop()
                stack.append(a + b)
                pc += 1

            elif op == SUB:
                b = stack.pop()
                a = stack.pop()
                stack.append(a - b)
                pc += 1

            elif op == MUL:
                b = stack.pop()
                a = stack.pop()
                stack.append(a * b)
                pc += 1

            elif op == DIV:
                b = stack.pop()
                a = stack.pop()
                if b == 0:
                    raise ZeroDivisionError("ділення на нуль в Oberih-програмі")
                stack.append(a / b)
                pc += 1

            elif op == CMP:
                b = stack.pop()
                a = stack.pop()
                if arg == "==":
                    result = a == b
                elif arg == "!=":
                    result = a != b
                elif arg == "<":
                    result = a < b
                elif arg == ">":
                    result = a > b
                elif arg == "<=":
                    result = a <= b
                elif arg == ">=":
                    result = a >= b
                else:
                    raise RuntimeError(f"Невідомий оператор порівняння: {arg}")
                stack.append(result)
                pc += 1

            elif op == JUMP:
                pc = arg

            elif op == JUMP_IF_FALSE:
                cond = stack.pop()
                pc = arg if not cond else pc + 1

            elif op == RETURN:
                return stack.pop()

            elif op == CALL_FUNCTION:
                fn_name, argc = arg
                call_args = [stack.pop() for _ in range(argc)][::-1]
                from evaluator import call_by_name  # лінивий імпорт - уникаємо циклу
                result = call_by_name(fn_name, call_args, None)
                stack.append(result)
                pc += 1

            elif op == TAIL_CALL_FUNCTION:
                fn_name, argc = arg
                call_args = [stack.pop() for _ in range(argc)][::-1]
                import evaluator

                bytecode = evaluator._bytecode_cache.get(fn_name)
                if fn_name not in evaluator._bytecode_cache:
                    fn = evaluator.REGISTRY.get(fn_name)
                    if fn is not None and not fn["is_resilient"]:
                        try:
                            import bytecode_compiler
                            bytecode = bytecode_compiler.compile_function_body(fn["body"])
                            evaluator._bytecode_cache[fn_name] = bytecode
                        except Exception:
                            evaluator._bytecode_cache[fn_name] = None
                            bytecode = None

                if bytecode is not None:
                    # Справжній хвостовий виклик: НЕ рекурсуємо в Python -
                    # перезапускаємо зовнішній цикл з новим кадром.
                    fn_params = evaluator.REGISTRY[fn_name]["params"]
                    code = bytecode
                    variables = dict(zip(fn_params, call_args))
                    jumped_to_new_frame = True
                    break
                else:
                    # Ціль недоступна для байткоду (resilient/builtin/tree-
                    # walking-only) - звичайний виклик, без TCO, але коректний.
                    from evaluator import call_by_name
                    return call_by_name(fn_name, call_args, None)

            elif op == POP:
                stack.pop()
                pc += 1

            else:
                raise RuntimeError(f"Невідома інструкція байткоду: {op}")

        if jumped_to_new_frame:
            continue  # зовнішній while True продовжує з новим кадром

        return None  # код закінчився без RETURN/TAIL_CALL_FUNCTION
