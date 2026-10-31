import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

import threading
import time
from lark import Lark
from module_loader import parse_with_imports
from evaluator import load_program, call_user_function
from interpreter import network, adaptive_backpressure_snapshot, SaturationRejectedError, NetworkFailure

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn adaptiveCall(id: String) -> Result<String, Error>
    deadline(30s)
    retryBudget(1)
    saturating(min: 2, max: 10)
{
    return http.get("/adaptive/" + id)
}
"""

tree = parse_with_imports_helper = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ФАЗА 1: сервіс відповідає ШВИДКО (20ms) - ліміт має РОСТИ")
print("=" * 70)
network.configure_delay("http:adaptive", 0.02)

results = []
lock = threading.Lock()


def worker(i):
    try:
        call_user_function("adaptiveCall", [f"item-{i}"])
        with lock:
            results.append("OK")
    except (SaturationRejectedError, NetworkFailure) as e:
        with lock:
            results.append("REJECTED")


# Робимо кілька хвиль по 8 паралельних викликів - ліміт стартує з 2, має рости
for wave in range(4):
    results.clear()
    threads = [threading.Thread(target=worker, args=(i,)) for i in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    snap = adaptive_backpressure_snapshot()
    limit = snap.get("adaptiveCall", {}).get("limit")
    ok_count = results.count("OK")
    print(f"Хвиля {wave+1}: успішних {ok_count}/8, поточний ліміт: {limit:.2f}")

print()
print("=" * 70)
print("ФАЗА 2: сервіс раптово стає ПОВІЛЬНИМ (500ms, у 25 разів довше)")
print("Очікується: ліміт має ЗНИЗИТИСЬ через виявлену затримку")
print("=" * 70)
network.configure_delay("http:adaptive", 0.5)

for wave in range(3):
    results.clear()
    threads = [threading.Thread(target=worker, args=(i,)) for i in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    snap = adaptive_backpressure_snapshot()
    limit = snap.get("adaptiveCall", {}).get("limit")
    ok_count = results.count("OK")
    rejected_count = results.count("REJECTED")
    print(f"Хвиля {wave+1}: успішних {ok_count}/8, відхилено {rejected_count}, поточний ліміт: {limit:.2f}")

final_limit = adaptive_backpressure_snapshot()["adaptiveCall"]["limit"]
print()
print(f">>> Ліміт знизився після виявлення затримки (був вищий у фазі 1): "
      f"{'ТАК' if final_limit < 10 else 'ПРОВАЛ'}")
