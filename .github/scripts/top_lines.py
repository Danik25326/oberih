#!/usr/bin/env python3
"""Витягує з виводу `callgrind_annotate --auto=yes` рядки вихідного коду з найбільшою
кількістю виконаних інструкцій процесора (Ir) — тобто показує, ДЕ саме йде час.

Використання: top_lines.py <вивід_callgrind_annotate.txt> [скільки_рядків] [фільтр_файлу]
"""
import re
import sys

COST = re.compile(r"^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s?(.*)$")
FILE = re.compile(r"^-- Auto-annotated source:\s+(.+?)\s*$")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    path = sys.argv[1]
    top = int(sys.argv[2]) if len(sys.argv) > 2 else 40
    only = sys.argv[3] if len(sys.argv) > 3 else ""

    current = ""
    rows = []
    total = 0
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            m = FILE.match(line)
            if m:
                current = m.group(1)
                continue
            m = COST.match(line)
            if not m or not current:
                continue
            ir = int(m.group(1).replace(",", ""))
            total += ir
            if only and only not in current:
                continue
            rows.append((ir, float(m.group(2)), current, m.group(3).strip()))

    rows.sort(reverse=True)
    print(f"Найдорожчі рядки коду за кількістю інструкцій процесора (всього проаналізовано {total:,}):\n")
    print(f"{'інструкцій':>15} {'%':>6}  файл: рядок коду")
    print("-" * 100)
    for ir, pct, fname, text in rows[:top]:
        short = fname.split("/src/")[-1] if "/src/" in fname else fname
        print(f"{ir:>15,} {pct:>6.2f}  {short}: {text[:90]}")


if __name__ == "__main__":
    main()
