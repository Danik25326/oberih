"""
Інтерактивна консоль (REPL) для Oberih.

Використовує ту саму граматику, що й повний компілятор, але з окремою
точкою входу "stmt" (Lark підтримує кілька start-символів в одній
граматиці) - дозволяє вводити вирази/let/print прямо в консолі, без
обгортання у fn main() { ... }.

Чесне обмеження: не можна оголошувати fn/struct/enum прямо в REPL - лише
вирази та прості інструкції (let, return, if/while/for, print). Це
обмеження v0.1, не архітектурна стеля.
"""

import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from lark import Lark
from evaluator import Environment, eval_expr, exec_block, ReturnValue, _format_value

_GRAMMAR_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grammar.lark")


def run_repl():
    with open(_GRAMMAR_PATH, encoding="utf-8") as f:
        grammar_src = f.read()

    parser = Lark(grammar_src, parser="lalr", start=["start", "stmt"], propagate_positions=True)
    env = Environment()

    print("Oberih REPL v0.3 - введи вираз чи інструкцію, 'exit' для виходу")
    print("(fn/struct/enum поки не підтримуються в REPL - лише вирази/let/if/while/for)")
    print()

    while True:
        try:
            line = input("oberih> ")
        except (EOFError, KeyboardInterrupt):
            print()
            break

        stripped = line.strip()
        if stripped in ("exit", "quit"):
            break
        if not stripped:
            continue

        try:
            tree = parser.parse(line, start="stmt")
        except Exception as e:
            print(f"Помилка синтаксису: {e}")
            continue

        try:
            if tree.data == "let_stmt":
                name = str(tree.children[0])
                val = eval_expr(tree.children[1], env)
                env.set(name, val)
                print(f"{name} = {_format_value(val)}")
            elif tree.data == "assign_stmt":
                name = str(tree.children[0])
                val = eval_expr(tree.children[1], env)
                env.set(name, val)
                print(f"{name} = {_format_value(val)}")
            elif tree.data == "expr_stmt":
                val = eval_expr(tree.children[0], env)
                if val is not None:
                    print(_format_value(val))
            else:
                try:
                    exec_block([tree], env)
                except ReturnValue as rv:
                    print(_format_value(rv.value))
        except Exception as e:
            print(f"Помилка виконання: {type(e).__name__}: {e}")


if __name__ == "__main__":
    run_repl()
