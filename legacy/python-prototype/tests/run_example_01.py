import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

"""
Перший реальний запуск .obh файлу через повний конвеєр:
текст -> Lark parser -> AST -> evaluator -> результат.

Це той самий сценарій, що й demo_shared_budget.py, але тепер логіка
fetchOrder/fetchUser/fetchPaymentStatus/getOrderSummary НЕ написана вручну
на Python - вона повністю зчитується з examples/01_microservices_chain.obh.
"""

from lark import Lark
from evaluator import load_program, call_user_function
from interpreter import network

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

with open(os.path.join(_ROOT, "examples/01_microservices_chain.obh")) as f:
    source = f.read()

tree = parser.parse(source)
load_program(tree)
print("Програму завантажено. Функції в реєстрі:", list(__import__("evaluator").REGISTRY.keys()))

print()
print("=" * 70)
print("ТЕСТ 1: усі сервіси постійно падають -> має спрацювати fallback")
print("=" * 70)
network.configure_failures("http:orders", 999)
network.configure_failures("http:users", 999)
network.configure_failures("http:payments", 999)
network.call_log.clear()

result = call_user_function("getOrderSummary", ["order-123"])
print(f"Результат: {result}")
print(f"Реальних викликів: {len(network.call_log)} -> {network.call_log}")

print()
print("=" * 70)
print("ТЕСТ 2: кожен сервіс падає рівно 1 раз -> має вдатись без fallback")
print("=" * 70)
network.configure_failures("http:orders", 1)
network.configure_failures("http:users", 1)
network.configure_failures("http:payments", 1)
network.call_log.clear()

result = call_user_function("getOrderSummary", ["order-456"])
print(f"Результат: {result}")
print(f"Реальних викликів: {len(network.call_log)} -> {network.call_log}")
