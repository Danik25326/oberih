import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

"""
Доводимо, що idempotent і circuitBreaker реально впливають на поведінку,
а не просто парсяться без ефекту.
"""

from lark import Lark
from evaluator import load_program, call_user_function
from typechecker import check_program
from interpreter import (
    network, circuit_breaker_snapshot, CircuitOpenError,
    ResilienceExhausted, NetworkFailure,
)

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

with open(os.path.join(_ROOT, "examples/02_payment_gateway.obh")) as f:
    source = f.read()

import evaluator
tree = parser.parse(source)
load_program(tree)
errors = check_program(evaluator.REGISTRY)
print("Помилки компіляції:", errors or "немає (ОК)")

print()
print("=" * 70)
print("ТЕСТ A: Idempotent - повторний виклик з тим самим ключем")
print("=" * 70)
network.call_log.clear()
network.fail_counts.clear()

r1 = call_user_function("chargeCard", [100, "key-A"])
print(f"Перший виклик chargeCard(100, 'key-A'): {r1}")
print(f"Мережевих викликів після 1-го разу: {len(network.call_log)}")

r2 = call_user_function("chargeCard", [100, "key-A"])
print(f"Другий виклик chargeCard(100, 'key-A') (той самий ключ!): {r2}")
print(f"Мережевих викликів після 2-го разу: {len(network.call_log)}")
print(f">>> Якщо кількість НЕ зросла - idempotent реально захистив від подвійного списання: "
      f"{'ТАК' if len(network.call_log) == 1 else 'ПРОВАЛ'}")

print()
print("=" * 70)
print("ТЕСТ B: Circuit Breaker - 5 збоїв поспіль мають відкрити 'запобіжник'")
print("=" * 70)
network.call_log.clear()
network.configure_failures("paymentGateway", 999)  # сервіс постійно падає

for i in range(7):
    key = f"key-B-{i}"
    try:
        call_user_function("chargeCard", [50, key])
        print(f"Виклик {i+1} (ключ {key}): УСПІХ (несподівано)")
    except (ResilienceExhausted, NetworkFailure) as e:
        state = circuit_breaker_snapshot().get("chargeCard", {})
        kind = "CircuitOpenError (breaker заблокував)" if isinstance(e, CircuitOpenError) else type(e).__name__
        print(f"Виклик {i+1} (ключ {key}): {kind}, "
              f"стан breaker: {state.get('state')}, збоїв: {state.get('failures')}")

print()
print(f"Реальних мережевих викликів зроблено: {len(network.call_log)} з 7 спроб")
print(f">>> Якщо менше 7 - breaker реально заблокував частину викликів: "
      f"{'ТАК' if len(network.call_log) < 7 else 'ПРОВАЛ'}")
