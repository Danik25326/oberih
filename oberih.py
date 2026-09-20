#!/usr/bin/env python3
"""
Oberih CLI - командний рядок для запуску .obh файлів.

Використання:
    python3 oberih.py run path/to/file.obh

Програма повинна містити функцію `main()` - саме вона виконується першою,
так само як у C, Go, Rust.
"""

import sys
import os
sys.setrecursionlimit(10000)  # кожен рівень рекурсії Oberih = кілька Python-фреймів
from lark import Lark
from evaluator import load_program, call_user_function, REGISTRY
import evaluator
from typechecker import (
    check_program, type_check_program, check_emergency_fallback_no_io,
    check_agent_budget_required, check_private_visibility,
)
import interpreter
from module_loader import parse_with_imports, ObirihCompileError


def load_grammar():
    grammar_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        return Lark(f.read(), parser="lalr", propagate_positions=True)


def run_file(path, use_mock=False):
    parser = load_grammar()
    try:
        tree = parse_with_imports(path, parser)
    except ObirihCompileError as e:
        print(str(e), file=sys.stderr)
        sys.exit(1)
    except Exception as e:
        print(f"Помилка синтаксису в {path}:\n{e}", file=sys.stderr)
        sys.exit(1)

    load_program(tree)

    interpreter.HTTP_MODE = "mock" if use_mock else "real"

    errors = (
        check_program(REGISTRY)
        + type_check_program(REGISTRY, evaluator.STRUCTS)
        + check_emergency_fallback_no_io(REGISTRY)
        + check_agent_budget_required(REGISTRY)
        + check_private_visibility(REGISTRY)
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
    args = sys.argv[1:]
    if not args:
        print("Використання: python3 oberih.py run [--mock] <файл.obh>", file=sys.stderr)
        print("           або: python3 oberih.py install", file=sys.stderr)
        sys.exit(1)

    if args[0] == "install":
        from package_manager import install_all
        project_dir = args[1] if len(args) > 1 else "."
        install_all(project_dir)
        return

    if args[0] == "replay":
        if len(args) < 3:
            print("Використання: python3 oberih.py replay <файл.obh> <функція> [аргументи...]", file=sys.stderr)
            sys.exit(1)
        interpreter.REPLAY_MODE = True
        parser = load_grammar()
        try:
            tree = parse_with_imports(args[1], parser)
        except ObirihCompileError as e:
            print(str(e), file=sys.stderr)
            sys.exit(1)
        load_program(tree)
        fn_name = args[2]
        fn_args = args[3:]
        print(f"[REPLAY MODE] Виконую '{fn_name}' виключно з журналу durable execution "
              f"- реальні I/O заблоковано.\n")
        try:
            result = call_user_function(fn_name, fn_args)
            print(f"\nРезультат replay: {result}")
        except Exception as e:
            print(f"\nReplay зупинено: {type(e).__name__}: {e}", file=sys.stderr)
            sys.exit(1)
        return

    if args[0] == "incidents":
        from interpreter import list_recent_incidents
        import json as _json
        files = list_recent_incidents(limit=20)
        if not files:
            print("Немає збережених post-mortem звітів (.oberih_incidents/ порожня).")
            return
        for path in files:
            with open(path, encoding="utf-8") as f:
                report = _json.load(f)
            print(f"--- {report['function']} @ {report['timestamp']} ---")
            print(f"  Помилка: {report['last_error']}")
            print(f"  Залишок спроб: {report['retries_left_at_failure']}, "
                  f"залишок часу: {report['deadline_remaining_seconds']}s")
            print(f"  Файл: {path}")
            print()
        return

    if args[0] == "repl":
        from repl import run_repl
        run_repl()
        return

    if args[0] == "explain":
        from explain import run_explain
        rest = args[1:]
        forecast = "--forecast" in rest
        if forecast:
            rest.remove("--forecast")
        if len(rest) < 1:
            print("Використання: python3 oberih.py explain <файл.obh> [функція] [--forecast]", file=sys.stderr)
            sys.exit(1)
        fn_name = rest[1] if len(rest) > 1 else None
        run_explain(rest[0], fn_name, forecast=forecast)
        return

    if args[0] != "run":
        print("Використання: python3 oberih.py run [--mock] <файл.obh>", file=sys.stderr)
        print("           або: python3 oberih.py install", file=sys.stderr)
        sys.exit(1)

    args = args[1:]
    use_mock = False
    if "--mock" in args:
        use_mock = True
        args.remove("--mock")

    if not args:
        print("Використання: python3 oberih.py run [--mock] <файл.obh>", file=sys.stderr)
        sys.exit(1)

    run_file(args[0], use_mock=use_mock)


if __name__ == "__main__":
    main()
