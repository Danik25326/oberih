#!/usr/bin/env python3
"""Порівнює швидкість двох збірок oberih на ОДНІЙ машині.

Використання: compare_bench.py <старий_бінарник> <новий_бінарник> <bench.obh> [кількість_запусків]

Запуски чергуються (старий, новий, новий, старий, ...), щоб дрейф швидкості машини
(сусідні віртуалки, температура) не перекошував результат на користь однієї зі сторін.
Для кожного тесту виводяться мінімум і медіана та їхнє відношення «новий / старий»:
менше 1.00 — новий швидший. Розкид між запусками теж показується: якщо він порівнянний
із різницею, результату довіряти не можна.
"""
import os
import re
import statistics
import subprocess
import sys

LINE = re.compile(r"^(\d+)\s+(.+?):\s+(\d+)\s*мс\s*$")


def run_once(binary, bench):
    proc = subprocess.run([binary, "run", bench], capture_output=True, text=True, timeout=600)
    if proc.returncode != 0:
        print(proc.stdout)
        print(proc.stderr, file=sys.stderr)
        sys.exit(f"{binary} завершився з кодом {proc.returncode}")
    result = {}
    for line in proc.stdout.splitlines():
        m = LINE.match(line.strip())
        if m:
            result[int(m.group(1))] = (m.group(2).strip(), int(m.group(3)))
    if not result:
        sys.exit("у виводі бенчмарка не знайдено жодного рядка виду '<n> назва: <число> мс'")
    return result


def main():
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    old, new, bench = sys.argv[1], sys.argv[2], sys.argv[3]
    runs = int(sys.argv[4]) if len(sys.argv) > 4 else 7

    names = {}
    samples = {"old": {}, "new": {}}
    # один прогрів кожного: перший запуск холодний (файловий кеш, частота процесора)
    run_once(old, bench)
    run_once(new, bench)

    for i in range(runs):
        order = [("old", old), ("new", new)] if i % 2 == 0 else [("new", new), ("old", old)]
        for tag, binary in order:
            for idx, (name, ms) in run_once(binary, bench).items():
                names[idx] = name
                samples[tag].setdefault(idx, []).append(ms)

    rows = []
    for idx in sorted(names):
        o, n = samples["old"].get(idx, []), samples["new"].get(idx, [])
        if not o or not n:
            continue
        o_min, n_min = min(o), min(n)
        o_med, n_med = statistics.median(o), statistics.median(n)
        spread = max(max(o) - min(o), max(n) - min(n))
        rows.append((idx, names[idx], o_min, n_min, o_med, n_med, spread))

    header = f"{'тест':<44} {'старий мін':>10} {'новий мін':>10} {'нов/стар':>9} {'мед.стар':>9} {'мед.нов':>8} {'нов/стар':>9} {'розкид':>7}"
    lines = [header, "-" * len(header)]
    md = ["| тест | старий (мін) | новий (мін) | нов/стар (мін) | нов/стар (медіана) | розкид, мс |",
          "|---|---:|---:|---:|---:|---:|"]
    regressions = []
    for idx, name, o_min, n_min, o_med, n_med, spread in rows:
        r_min = n_min / o_min if o_min else float("nan")
        r_med = n_med / o_med if o_med else float("nan")
        lines.append(f"{idx} {name:<42} {o_min:>10} {n_min:>10} {r_min:>9.2f} {o_med:>9.0f} {n_med:>8.0f} {r_med:>9.2f} {spread:>7}")
        md.append(f"| {idx} {name} | {o_min} | {n_min} | {r_min:.2f} | {r_med:.2f} | {spread} |")
        if r_min > 1.20 and r_med > 1.20:
            regressions.append(f"{idx} {name}: нов/стар = {r_min:.2f} (мін), {r_med:.2f} (медіана)")

    text = "\n".join(lines)
    print(f"Запусків на кожну збірку: {runs} (чергуються). Значення < 1.00 — новий швидший.\n")
    print(text)
    if regressions:
        print("\nМожлива регресія (повільніше більш ніж на 20% і за мінімумом, і за медіаною):")
        for r in regressions:
            print("  - " + r)
            print(f"::warning::{r}")

    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(f"### Порівняння швидкості ({runs} чергованих запусків на збірку)\n\n")
            f.write("\n".join(md) + "\n\n")
            f.write("`нов/стар` < 1.00 означає, що нова версія швидша. Розкид — найбільша різниця між "
                    "запусками однієї збірки: якщо він порівнянний із різницею збірок, висновку робити не можна.\n")
            if regressions:
                f.write("\n**Можлива регресія:**\n" + "\n".join(f"- {r}" for r in regressions) + "\n")


if __name__ == "__main__":
    main()
