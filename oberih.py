#!/usr/bin/env python3
"""
Oberih CLI - командний рядок для запуску .obh файлів.

Використання:
    python3 oberih.py run path/to/file.obh

Програма повинна містити функцію `main()` - саме вона виконується першою,
так само як у C, Go, Rust.
"""

import sys
from lark import Lark
from evaluator import load_program, call_user_function, REGISTRY
from typechecker import check_program, type_check_program, check_emergency_fallback_no_io


def load_grammar():
    with open(__file__.replace("oberih.py", "grammar.lark")) as f:
        return Lark(f.read(), parser="lalr", propagate_positions=True)


def run_file(path):
    with open(path, encoding="utf-8") as f:
        source = f.read()

    parser = load_grammar()
    try:
        tree = parser.parse(source)
    except Exception as e:
        print(f"Помилка синтаксису в {path}:\n{e}", file=sys.stderr)
        sys.exit(1)

    load_program(tree)

    errors = (
        check_program(REGISTRY)
        + type_check_program(REGISTRY)
        + check_emergency_fallback_no_io(REGISTRY)
    )
    if errors:
        print(f"Помилка компіляції ({path}):", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        sys.exit(1)

    if "main" not in REGISTRY:
        print(
            f"Помилка: у {path} немає функції main(). "
            f"Точка входу програми повинна називатись main().",
            file=sys.stderr,
        )
        sys.exit(1)

    try:
        call_user_function("main", [])
    except Exception as e:
        print(f"Помилка виконання: {type(e).__name__}: {e}", file=sys.stderr)
        sys.exit(1)


def main():
    if len(sys.argv) < 3 or sys.argv[1] != "run":
        print("Використання: python3 oberih.py run <файл.obh>", file=sys.stderr)
        sys.exit(1)
    run_file(sys.argv[2])


if __name__ == "__main__":
    main()
