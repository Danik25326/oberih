import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

import threading
import time
from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network, BulkheadRejectedError, NetworkFailure

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn limitedResource(x: String) -> Result<String, Error>
    deadline(3s)
    retryBudget(1)
    bulkhead(maxConcurrent: 2)
{
    return http.get("/slow-resource/" + x)
}
"""

tree = parser.parse(source)
load_program(tree)

# Штучна затримка, щоб виклики РЕАЛЬНО перекривались у часі
network.configure_delay("http:slow-resource", 0.3)

print("=" * 70)
print("ТЕСТ: bulkhead(maxConcurrent: 2) - запускаємо 5 паралельних викликів")
print("Очікується: тільки 2 виконуються одночасно, решта отримують відмову")
print("=" * 70)

results = []
lock = threading.Lock()


def worker(i):
    try:
        call_user_function("limitedResource", [f"item-{i}"])
        with lock:
            results.append("OK")
    except NetworkFailure as e:
        kind = "BULKHEAD_REJECTED" if isinstance(e, BulkheadRejectedError) else type(e).__name__
        with lock:
            results.append(kind)


threads = [threading.Thread(target=worker, args=(i,)) for i in range(5)]
for t in threads:
    t.start()
for t in threads:
    t.join()

print(f"Результати 5 паралельних викликів: {results}")
ok_count = results.count("OK")
rejected_count = results.count("BULKHEAD_REJECTED")
print(f"Успішних: {ok_count}, відхилених bulkhead: {rejected_count}")
print(f">>> Рівно 2 успішних (maxConcurrent=2), решта відхилені: "
      f"{'ТАК' if ok_count == 2 and rejected_count == 3 else 'ПРОВАЛ (можливий race, спробуй ще раз)'}")
