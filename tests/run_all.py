#!/usr/bin/env python3
"""
Запускає всі тести проєкту Oberih послідовно і показує підсумок.

Використання:
    python3 tests/run_all.py
"""

import subprocess
import sys
import os

TESTS_DIR = os.path.dirname(__file__)

TEST_FILES = [
    "demo_shared_budget.py",
    "run_example_01.py",
    "run_example_02.py",
    "run_example_timeout.py",
    "run_example_emergency.py",
    "run_example_ratelimit.py",
    "run_example_bulkhead.py",
    "run_example_hedging.py",
    "run_example_lsp.py",
    "run_example_adaptive.py",
]


def main():
    results = []
    for fname in TEST_FILES:
        path = os.path.join(TESTS_DIR, fname)
        print(f"\n{'=' * 70}\nЗАПУСК: {fname}\n{'=' * 70}")
        proc = subprocess.run([sys.executable, path], capture_output=True, text=True)
        print(proc.stdout)
        if proc.returncode != 0:
            print(proc.stderr, file=sys.stderr)
        results.append((fname, proc.returncode == 0))

    print(f"\n{'=' * 70}\nПІДСУМОК\n{'=' * 70}")
    for fname, ok in results:
        status = "OK" if ok else "ПРОВАЛ"
        print(f"  [{status}] {fname}")

    failed = [f for f, ok in results if not ok]
    if failed:
        print(f"\n{len(failed)} з {len(results)} тестів провалились.")
        sys.exit(1)
    else:
        print(f"\nУсі {len(results)} тестів пройшли успішно.")


if __name__ == "__main__":
    main()
