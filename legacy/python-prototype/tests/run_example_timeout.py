import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network, ResilienceExhausted

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn slowCall(x: String) -> Result<String, Error>
    deadline(3s)
    retryBudget(3)
    timeout(200ms)
    fallback("timed-out-fallback")
{
    return http.get("/slow/" + x)
}
"""

tree = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ТЕСТ: сервіс відповідає за 500ms, а timeout виставлено 200ms")
print("=" * 70)
network.configure_delay("http:slow", 0.5)  # 500ms - довше за timeout(200ms)
network.call_log.clear()

result = call_user_function("slowCall", ["x"])
print(f"Результат: {result!r}")
print(f"Реальних викликів: {len(network.call_log)}")
print(f">>> Fallback спрацював через timeout: "
      f"{'ТАК' if result == 'timed-out-fallback' else 'НІ, ПРОВАЛ'}")

print()
print("=" * 70)
print("ТЕСТ: сервіс відповідає за 50ms, а timeout виставлено 200ms (має вдатись)")
print("=" * 70)
network.configure_delay("http:slow", 0.05)
network.call_log.clear()

result = call_user_function("slowCall", ["y"])
print(f"Результат: {result!r}")
print(f">>> Реальна відповідь отримана (не fallback): "
      f"{'ТАК' if result != 'timed-out-fallback' else 'НІ, ПРОВАЛ'}")
