import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network, RateLimitedError, ResilienceExhausted, NetworkFailure

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn limitedCall(x: String) -> Result<String, Error>
    deadline(2s)
    retryBudget(1)
    rateLimit(3, per: 1s)
{
    return http.get("/limited/" + x)
}
"""

tree = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ТЕСТ: rateLimit(3, per: 1s) - робимо 5 викликів підряд миттєво")
print("Очікується: перші 3 успішні, наступні 2 відхилені лімітом")
print("=" * 70)

network.call_log.clear()
results = []
for i in range(5):
    try:
        r = call_user_function("limitedCall", [f"item-{i}"])
        results.append("OK")
    except (ResilienceExhausted, NetworkFailure) as e:
        kind = "RATE_LIMITED" if isinstance(e, RateLimitedError) else type(e).__name__
        results.append(kind)

print(f"Результати 5 викликів: {results}")
print(f"Реальних мережевих викликів: {len(network.call_log)}")
print(f">>> Перші 3 успішні, решта відхилені: "
      f"{'ТАК' if results[:3] == ['OK','OK','OK'] and 'RATE_LIMITED' in results[3:] else 'ПРОВАЛ'}")
