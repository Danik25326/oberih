import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

import time
from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn hedgedCall(x: String) -> Result<String, Error>
    deadline(3s)
    retryBudget(1)
    hedging(after: 150ms)
{
    return http.get("/hedged/" + x)
}
"""

tree = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ТЕСТ A: сервіс відповідає швидко (50ms) - hedging НЕ повинен спрацювати")
print("=" * 70)
network.configure_delay("http:hedged", 0.05)
start = time.monotonic()
result = call_user_function("hedgedCall", ["a"])
elapsed = time.monotonic() - start
print(f"Результат: {result}, час: {elapsed*1000:.0f}ms")
print(f">>> Швидко, без затримки на очікування другої спроби: "
      f"{'ТАК' if elapsed < 0.15 else 'ПРОВАЛ (задовго)'}")

print()
print("=" * 70)
print("ТЕСТ B: сервіс відповідає повільно (1000ms) - hedging МАЄ запустити дублюючу спробу")
print("Але дублююча спроба відповідає швидко (50ms) - тому загальний час має бути ~150+50=200ms, не 1000ms")
print("=" * 70)


call_count = {"n": 0}
original_call = network.call

def counting_call(service_name):
    call_count["n"] += 1
    this_call = call_count["n"]
    # Перша реальна спроба - повільна; друга (дублююча) - швидка
    delay = 1.0 if this_call == 1 else 0.05
    network.configure_delay(service_name, delay)
    return original_call(service_name)

network.call = counting_call

start = time.monotonic()
result = call_user_function("hedgedCall", ["b"])
elapsed = time.monotonic() - start
print(f"Результат: {result}, час: {elapsed*1000:.0f}ms, реальних викликів: {call_count['n']}")
print(f">>> Дублююча спроба врятувала загальний час (менше 1000ms): "
      f"{'ТАК' if elapsed < 0.5 else 'ПРОВАЛ'}")
print(f">>> Було зроблено 2 реальних виклики (перша + дублююча): "
      f"{'ТАК' if call_count['n'] == 2 else 'ПРОВАЛ'}")
