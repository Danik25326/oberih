#!/usr/bin/env python3
"""Soak-тест: тривале навантаження на HTTP-сервер oberih з вимірюванням витоків.

Що робить
  1. Запускає `oberih run bench/soak/server.obh` (маршрути вправляють алокації, JSON, тіла
     різного розміру, помилки обробника, resilient fn, логи, метрики, цикли, spawn).
  2. Подає змішане навантаження + «ворожих» клієнтів (обірвані заголовки, сміття, завеликі
     заголовки й тіла, обірване тіло, chunked).
  3. Кожні N секунд знімає: RSS, кількість потоків і відкритих дескрипторів процесу (Linux),
     live-об'єкти GC (/stats), запити в обробці й лічильник запитів (/metrics).
  4. Після навантаження чекає спокою, знімає знімок ще раз і ПЕРЕВІРЯЄ:
       - нахил RSS після прогріву (МіБ/год) і стабільність (R²) — головна ознака витоку;
       - потоки й дескриптори повернулись до початкових (витік з'єднань/потоків);
       - live-об'єкти GC не ростуть на запит (витік структур мови);
       - запити в обробці == 0, сервер відповідає;
       - частка неочікуваних відповідей/таймаутів, деградація p99 від початку до кінця;
       - SIGTERM → коректна зупинка з кодом 0.
  5. Пише JSON/CSV та Markdown-підсумок ($GITHUB_STEP_SUMMARY, якщо задано).

Сценарії (--scenario)
  steady         — типове навантаження; ВСІ перевірки мають пройти.
  known          — додає /cycle і /spawn: відомі межі (цикли й задачі spawn не звільняються).
                   Перевірки росту лише повідомляються (info), не валять прогін.
  leak-selftest  — сервер із навмисною течею (/leak); харнес МАЄ це виявити (перевірка самого
                   харнеса: прогін вважається успішним, лише якщо витік знайдено).

Лише стандартна бібліотека Python 3.8+. Нативно працює на Linux (потрібен /proc); на macOS
міряється лише RSS. Windows не підтримується.
"""
import argparse
import csv
import http.client
import json
import os
import platform
import random
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
HOST = "127.0.0.1"


# ------------------------------------------------------------------ навантаження
def mix(scenario, only=None):
    """(метод, шлях-шаблон, вага, очікувані статуси, розмір тіла або None)"""
    m = [
        ("GET", "/health", 10, {200}, None),
        ("GET", "/work?n={r60}", 35, {200}, None),
        ("POST", "/echo", 15, {200}, "body"),
        ("GET", "/fail", 2, {500}, None),
        ("GET", "/slow?ms=20", 5, {200}, None),
        ("GET", "/resil?n={r9}", 5, {200}, None),
        ("GET", "/log?n={r9}", 5, {200}, None),
        ("GET", "/count", 5, {200}, None),
        ("GET", "/metrics", 5, {200}, None),
        ("GET", "/stats", 2, {200}, None),
        ("GET", "/nope", 2, {404}, None),
    ]
    if scenario == "known":
        m += [("GET", "/cycle", 15, {200}, None), ("GET", "/spawn", 15, {200}, None)]
    if scenario == "leak-selftest":
        m += [("GET", "/leak", 80, {200}, None)]
    if only:   # діагностика: лише вказані маршрути (за префіксом шляху), без ворожих клієнтів
        m = [r for r in m if any(r[1].startswith(o) for o in only)]
        if not m:
            sys.exit("--only: жоден маршрут не підходить: %s" % only)
    return m


BODY_SIZES = [0, 100, 10_000, 200_000, 1_048_000]  # останній — майже на ліміті сервера (1 МіБ)


def abuse(port, kind, rng):
    """Ворожий клієнт. Повертає True, якщо відповідь (де вона очікується) коректна."""
    s = socket.create_connection((HOST, port), timeout=8)
    try:
        if kind == 0:      # обірвані заголовки
            s.sendall(b"GET /health HTTP/1.1\r\nHost: x\r\nX-Pa")
            return True
        if kind == 1:      # сміття
            s.sendall(bytes(rng.getrandbits(8) for _ in range(rng.randint(1, 300))))
            return True
        if kind == 2:      # завеликий заголовок -> 431
            s.sendall(b"GET /health HTTP/1.1\r\nX-Big: " + b"a" * 20000 + b"\r\n\r\n")
            return s.recv(64).startswith(b"HTTP/1.1 431")
        if kind == 3:      # завелике тіло -> 413
            s.sendall(b"POST /echo HTTP/1.1\r\nContent-Length: 2000000\r\n\r\n")
            return s.recv(64).startswith(b"HTTP/1.1 413")
        if kind == 4:      # з'єднався й одразу закрив
            return True
        if kind == 5:      # обірване тіло
            s.sendall(b"POST /echo HTTP/1.1\r\nContent-Length: 1000\r\n\r\nshort")
            return True
        if kind == 6:      # chunked -> 501
            s.sendall(b"POST /echo HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
            return s.recv(64).startswith(b"HTTP/1.1 501")
        return True
    except (socket.timeout, ConnectionError, OSError):
        return kind in (0, 1, 4, 5)  # для цих сервер міг просто закрити з'єднання
    finally:
        try:
            s.close()
        except OSError:
            pass


class Stats:
    def __init__(self):
        self.lock = threading.Lock()
        self.total = 0
        self.unexpected = Counter()
        self.lat = []          # (час від початку, секунди)

    def add(self, t, dt, ok, why=""):
        with self.lock:
            self.total += 1
            self.lat.append((t, dt))
            if not ok:
                self.unexpected[why] += 1


def worker(port, scenario, stop, t0, stats, interval, seed, only=None):
    rng = random.Random(seed)
    abuse_only = only == ["abuse"]
    table = mix(scenario, None if abuse_only else only)
    weights = [w for _, _, w, _, _ in table]
    next_t = time.monotonic()
    while not stop.is_set():
        next_t += interval
        delay = next_t - time.monotonic()
        if delay > 0:
            stop.wait(delay)
        elif delay < -2:       # відстали — не накопичуємо борг
            next_t = time.monotonic()
        if abuse_only or (not only and rng.random() < 0.04):   # ~4% — ворожі клієнти
            kind = rng.randint(0, 6)
            t = time.monotonic()
            try:
                ok = abuse(port, kind, rng)
            except Exception as e:                 # не вдалось навіть з'єднатись
                stats.add(t - t0, time.monotonic() - t, False, "abuse-connect:" + type(e).__name__)
                continue
            stats.add(t - t0, time.monotonic() - t, ok, "abuse-%d" % kind)
            continue
        method, path, _, expect, body = rng.choices(table, weights)[0]
        path = path.replace("{r60}", str(rng.randint(1, 60))).replace("{r9}", str(rng.randint(0, 9)))
        data = None
        if body:
            data = ("é" * 0 + "x" * rng.choice(BODY_SIZES)).encode()
        t = time.monotonic()
        try:
            c = http.client.HTTPConnection(HOST, port, timeout=15)
            c.request(method, path, body=data)
            r = c.getresponse()
            r.read()
            status = r.status
            c.close()
        except Exception as e:
            stats.add(t - t0, time.monotonic() - t, False, "%s %s" % (path.split("?")[0], type(e).__name__))
            continue
        stats.add(t - t0, time.monotonic() - t, status in expect, "%s -> %d" % (path.split("?")[0], status))


# ------------------------------------------------------------------ вимірювання процесу
def proc_sample(pid):
    out = {"rss_kb": None, "threads": None, "fds": None}
    try:
        with open("/proc/%d/status" % pid) as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    out["rss_kb"] = int(line.split()[1])
                elif line.startswith("Threads:"):
                    out["threads"] = int(line.split()[1])
        out["fds"] = len(os.listdir("/proc/%d/fd" % pid))
        return out
    except OSError:
        pass
    try:  # macOS та ін.
        r = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True, timeout=5)
        out["rss_kb"] = int(r.stdout.strip())
    except Exception:
        pass
    return out


def http_get(port, path, timeout=10):
    c = http.client.HTTPConnection(HOST, port, timeout=timeout)
    try:
        c.request("GET", path)
        r = c.getresponse()
        return r.status, r.read().decode("utf-8", "replace")
    finally:
        c.close()


def server_sample(port):
    out = {"gc_live": None, "in_flight": None, "requests": None}
    try:
        st, body = http_get(port, "/stats")
        if st == 200:
            out["gc_live"] = json.loads(body).get("live")
        st, body = http_get(port, "/metrics")
        if st == 200:
            total = 0.0
            for line in body.splitlines():
                if line.startswith("oberih_http_requests_total{"):
                    total += float(line.rsplit(" ", 1)[1])
                elif line.startswith("oberih_http_in_flight_requests "):
                    out["in_flight"] = float(line.split()[1])
            out["requests"] = total
    except Exception:
        pass
    return out


# ------------------------------------------------------------------ статистика
def linreg(xs, ys):
    """(нахил, R²) звичайної лінійної регресії."""
    n = len(xs)
    if n < 3:
        return 0.0, 0.0
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    syy = sum((y - my) ** 2 for y in ys)
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    if sxx == 0:
        return 0.0, 0.0
    slope = sxy / sxx
    r2 = (sxy * sxy) / (sxx * syy) if syy > 0 else 0.0
    return slope, r2


def pct(vals, p):
    if not vals:
        return 0.0
    v = sorted(vals)
    return v[min(len(v) - 1, int(len(v) * p))]


# ------------------------------------------------------------------ основний прогін
def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--bin", default=os.environ.get("OBERIH_BIN", "target/release/oberih"))
    ap.add_argument("--duration", type=float, default=300, help="тривалість навантаження, с")
    ap.add_argument("--scenario", choices=["steady", "known", "leak-selftest"], default="steady")
    ap.add_argument("--concurrency", type=int, default=8)
    ap.add_argument("--rps", type=float, default=150, help="цільова кількість запитів на секунду")
    ap.add_argument("--port", type=int, default=18090)
    ap.add_argument("--only", nargs="+", default=None, help="діагностика: лише ці маршрути (префікси шляху), напр. /work /echo; `abuse` — лише ворожі клієнти")
    ap.add_argument("--sample-interval", type=float, default=5)
    ap.add_argument("--warmup", type=float, default=0.25, help="частка прогріву, що ігнорується в нахилі")
    ap.add_argument("--max-mib-per-hour", type=float, default=None,
                    help="допустимий нахил RSS після прогріву (за замовчуванням 150 для прогонів < 30 хв, інакше 5)")
    ap.add_argument("--min-growth-mib", type=float, default=None,
                    help="мінімальний сумарний ріст RSS, щоб вважати його витоком (за замовчуванням 10 для коротких, 2 для довгих)")
    ap.add_argument("--min-r2", type=float, default=0.5, help="мінімальна «рівномірність» росту, щоб вважати його витоком")
    ap.add_argument("--max-live-per-1k", type=float, default=2.0, help="допустимий ріст GC-live на 1000 запитів")
    ap.add_argument("--max-unexpected-pct", type=float, default=0.5)
    ap.add_argument("--max-p99-degradation", type=float, default=3.0, help="p99 кінця / p99 початку")
    ap.add_argument("--out", default="soak-result.json")
    ap.add_argument("--csv", default="soak-samples.csv")
    ap.add_argument("--server-log", default=None)
    args = ap.parse_args()
    # Короткий прогін бачить «сходинки» високої водяної позначки алокатора (великі тіла запитів),
    # які за довгий час згасають; тож він ловить лише ГРУБІ витоки (від ≈0,5 КБ на запит), а
    # довгий — повільні. Пороги можна задати явно.
    short = args.duration < 1800
    if args.max_mib_per_hour is None:
        args.max_mib_per_hour = 150.0 if short else 5.0
    if args.min_growth_mib is None:
        args.min_growth_mib = 10.0 if short else 2.0

    binpath = os.path.abspath(args.bin)
    if not os.path.exists(binpath):
        sys.exit("не знайдено бінарник: %s (зберіть `cargo build --release` або вкажіть --bin)" % binpath)
    server = os.path.join(HERE, "server.obh")
    log_path = args.server_log or os.path.join(tempfile.gettempdir(), "oberih-soak-server.log")
    env = dict(os.environ, OBERIH_SOAK_PORT=str(args.port), OBERIH_LOG_FORMAT="json")
    if args.scenario == "leak-selftest":
        env["OBERIH_SOAK_LEAK"] = "1"

    try:   # якщо на порту хтось відповідає — ми б міряли ЧУЖИЙ сервер
        socket.create_connection((HOST, args.port), timeout=1).close()
        sys.exit("порт %d уже зайнятий — оберіть інший (--port)" % args.port)
    except OSError:
        pass

    print("== soak: сценарій=%s, %.0f с, %d потоків, ~%.0f запитів/с" %
          (args.scenario, args.duration, args.concurrency, args.rps), flush=True)
    logf = open(log_path, "wb")
    proc = subprocess.Popen([binpath, "run", server], env=env, stdout=logf, stderr=subprocess.STDOUT)
    try:
        # готовність
        deadline = time.time() + 30
        while True:
            if proc.poll() is not None:
                sys.exit("сервер завершився одразу (код %s), лог: %s" % (proc.returncode, log_path))
            try:
                if http_get(args.port, "/health", 2)[0] == 200:
                    break
            except Exception:
                pass
            if time.time() > deadline:
                sys.exit("сервер не став готовим за 30 с")
            time.sleep(0.2)
        time.sleep(1.5)
        base = dict(proc_sample(proc.pid), **server_sample(args.port))
        print("   базова лінія (спокій): RSS=%s КБ, потоків=%s, дескрипторів=%s" %
              (base["rss_kb"], base["threads"], base["fds"]), flush=True)

        # навантаження
        stop = threading.Event()
        stats = Stats()
        t0 = time.monotonic()
        interval = args.concurrency / args.rps
        workers = [threading.Thread(target=worker, args=(args.port, args.scenario, stop, t0, stats, interval, 1000 + i, args.only), daemon=True)
                   for i in range(args.concurrency)]
        for w in workers:
            w.start()

        samples = []
        server_died = None
        peak_threads = peak_fds = 0
        while time.monotonic() - t0 < args.duration:
            time.sleep(args.sample_interval)
            t = time.monotonic() - t0
            s = dict(proc_sample(proc.pid), **server_sample(args.port))
            s["t"] = t
            s["client_requests"] = stats.total
            samples.append(s)
            peak_threads = max(peak_threads, s["threads"] or 0)
            peak_fds = max(peak_fds, s["fds"] or 0)
            print("   t=%6.0f с  RSS=%7s КБ  потоків=%-4s fd=%-4s gc_live=%-7s запитів(клієнт)=%d  неочікуваних=%d" %
                  (t, s["rss_kb"], s["threads"], s["fds"], s["gc_live"], stats.total, sum(stats.unexpected.values())), flush=True)
            if proc.poll() is not None:
                server_died = proc.returncode
                break
        stop.set()
        for w in workers:
            w.join(timeout=20)

        # спокій
        time.sleep(5)
        waited = 0
        while waited < 20:
            fin = server_sample(args.port)
            if fin["in_flight"] in (0, 0.0, None) or fin["in_flight"] <= 1:   # 1 = сам запит /metrics
                break
            time.sleep(1)
            waited += 1
        final = dict(proc_sample(proc.pid), **server_sample(args.port))
        alive = False
        try:
            alive = http_get(args.port, "/health", 5)[0] == 200
        except Exception:
            pass

        # коректна зупинка
        t_stop = time.monotonic()
        proc.send_signal(signal.SIGTERM)
        try:
            code = proc.wait(timeout=40)
        except subprocess.TimeoutExpired:
            proc.kill()
            code = "timeout"
        stop_secs = time.monotonic() - t_stop
    finally:
        if proc.poll() is None:
            proc.kill()
        logf.close()

    # ------------------------------------------------------------ аналіз
    checks = []   # (назва, значення, межа, статус "pass"/"fail"/"info")
    known = args.scenario == "known"

    def add(name, value, limit, ok, info=False):
        checks.append((name, value, limit, "info" if info else ("pass" if ok else "fail")))

    warm_n = int(len(samples) * args.warmup)
    post = [s for s in samples[warm_n:] if s["rss_kb"] is not None]
    slope_mib_h = r2 = growth_mib = 0.0
    if len(post) >= 4:
        slope, r2 = linreg([s["t"] for s in post], [s["rss_kb"] for s in post])
        slope_mib_h = slope * 3600 / 1024
        growth_mib = (post[-1]["rss_kb"] - post[0]["rss_kb"]) / 1024
        leaking = slope_mib_h > args.max_mib_per_hour and r2 >= args.min_r2 and growth_mib > args.min_growth_mib
        add("RSS: нахил після прогріву, МіБ/год", "%.2f (R²=%.2f, ріст %.1f МіБ)" % (slope_mib_h, r2, growth_mib),
            "≤ %.0f, або R² < %.1f, або ріст ≤ %.0f МіБ" % (args.max_mib_per_hour, args.min_r2, args.min_growth_mib), not leaking)
    else:
        add("RSS: нахил після прогріву", "замало знімків (%d)" % len(post), "≥ 4", True, info=True)

    for key, label in (("threads", "потоки"), ("fds", "дескриптори")):
        b, f = base.get(key), final.get(key)
        if b is not None and f is not None:
            add("%s після спокою ≤ початкових + 3" % label, "%d → %d (пік %d)" % (b, f, peak_threads if key == "threads" else peak_fds),
                "≤ %d" % (b + 3), f <= b + 3)

    live = [(s["requests"], s["gc_live"]) for s in samples[warm_n:] if s["requests"] is not None and s["gc_live"] is not None]
    live_per_1k = 0.0
    if len(live) >= 4:
        sl, _ = linreg([a for a, _ in live], [b for _, b in live])
        live_per_1k = sl * 1000
        add("GC live-об'єкти, ріст на 1000 запитів", "%.2f" % live_per_1k, "≤ %.1f" % args.max_live_per_1k,
            live_per_1k <= args.max_live_per_1k, info=known)

    add("запитів у обробці після спокою", final.get("in_flight"), "≤ 1 (сам /metrics)",
        final.get("in_flight") is None or final["in_flight"] <= 1)
    add("сервер працював увесь прогін", "так" if server_died is None else "ЗАВЕРШИВСЯ з кодом %s" % server_died,
        "так", server_died is None)
    add("сервер відповідає після навантаження", alive, "True", alive)

    total = max(1, stats.total)
    unexpected = sum(stats.unexpected.values())
    add("неочікувані відповіді/таймаути", "%d з %d (%.3f%%)" % (unexpected, total, 100.0 * unexpected / total),
        "≤ %.1f%%" % args.max_unexpected_pct, 100.0 * unexpected / total <= args.max_unexpected_pct)

    q = args.duration / 4.0
    first = [d for t, d in stats.lat if t < q]
    last = [d for t, d in stats.lat if t >= 3 * q]
    p99a, p99b = pct(first, 0.99), pct(last, 0.99)
    if first and last:
        ratio = p99b / p99a if p99a > 0 else 1.0
        add("p99 латентності: кінець/початок", "%.0f мс → %.0f мс (×%.2f)" % (p99a * 1000, p99b * 1000, ratio),
            "≤ ×%.1f або < 100 мс" % args.max_p99_degradation, ratio <= args.max_p99_degradation or p99b < 0.1)

    add("коректна зупинка за SIGTERM", "код %s за %.1f с" % (code, stop_secs), "код 0, ≤ 25 с", code == 0 and stop_secs <= 25)

    failed = [c for c in checks if c[3] == "fail"]
    verdict_fail = bool(failed)
    selftest = args.scenario == "leak-selftest"
    rss_failed = any(c[0].startswith("RSS") and c[3] == "fail" for c in checks)
    if selftest:
        ok_overall = rss_failed        # харнес мусить виявити навмисну течу
    else:
        ok_overall = not verdict_fail

    # ------------------------------------------------------------ вивід
    icon = {"pass": "✅", "fail": "❌", "info": "ℹ️"}
    lines = ["## Soak-тест: %s — %s" % (args.scenario, "ПРОЙДЕНО" if ok_overall else "ПРОВАЛЕНО"), "",
             "Тривалість %.0f с, ~%d запитів (клієнт), %s, Python %s." % (args.duration, stats.total, platform.platform(), platform.python_version()), "",
             "| Перевірка | Значення | Межа | |", "|---|---|---|---|"]
    for name, val, lim, st in checks:
        lines.append("| %s | %s | %s | %s |" % (name, val, lim, icon[st]))
    if stats.unexpected:
        lines += ["", "Неочікувані (топ-8): " + ", ".join("`%s`×%d" % (k, v) for k, v in stats.unexpected.most_common(8))]
    if selftest:
        lines += ["", "_Самоперевірка харнеса: сервер навмисно тече; прогін успішний, лише якщо RSS-перевірка його виявила._"]
    if known:
        lines += ["", "_Сценарій `known`: ріст GC-live від /cycle і /spawn — відома межа мови (див. README), тут лише повідомляється._"]
    text = "\n".join(lines)
    print("\n" + text, flush=True)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
            f.write(text + "\n")

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump({"scenario": args.scenario, "ok": ok_overall, "duration": args.duration, "requests": stats.total,
                   "base": base, "final": final, "slope_mib_per_hour": slope_mib_h, "r2": r2,
                   "live_per_1k_requests": live_per_1k, "p99_first_s": p99a, "p99_last_s": p99b,
                   "checks": [{"name": n, "value": str(v), "limit": l, "status": s} for n, v, l, s in checks],
                   "unexpected": dict(stats.unexpected)}, f, ensure_ascii=False, indent=2)
    with open(args.csv, "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["t", "rss_kb", "threads", "fds", "gc_live", "in_flight", "server_requests", "client_requests"])
        for s in samples:
            w.writerow([round(s["t"], 1), s["rss_kb"], s["threads"], s["fds"], s["gc_live"], s["in_flight"], s["requests"], s["client_requests"]])

    if not ok_overall:
        try:
            with open(log_path, "rb") as f:
                f.seek(0, 2)
                f.seek(max(0, f.tell() - 3000))
                print("\n--- хвіст логу сервера (%s) ---\n%s" % (log_path, f.read().decode("utf-8", "replace")))
        except OSError:
            pass
    sys.exit(0 if ok_overall else 1)


if __name__ == "__main__":
    main()
