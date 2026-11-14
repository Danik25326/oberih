import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network, reset_durable_journal

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn fetchOrder(id: String) -> Result<Order, Error>
    retries(2)
{
    return http.get("/orders/" + id)
}

resilient fn chargeCard(id: String) -> Result<Receipt, Error>
    retries(2)
{
    return http.get("/charge/" + id)
}

resilient fn createShipment(id: String) -> Result<Shipment, Error>
    retries(2)
{
    return http.get("/shipment/" + id)
}

resilient fn processOrder(orderId: String) -> Result<String, Error>
    deadline(10s)
    retryBudget(10)
    durable
{
    let order = fetchOrder(orderId)?
    let payment = chargeCard(orderId)?
    let shipment = createShipment(orderId)?
    return "done"
}
"""

tree = parser.parse(source)
load_program(tree)

reset_durable_journal()  # чистий старт для тесту

print("=" * 70)
print("СИМУЛЯЦІЯ 1: процес виконує processOrder, але 'падає' на кроці 3")
print("(fetchOrder і chargeCard встигають ЗАВЕРШИТИСЬ і зберегтись у журнал)")
print("=" * 70)

# Хакаємо мережу так, щоб виклик до shipment-сервісу симулював крах процесу -
# не оброблювану помилку, що вилітає з усієї системи (як реальний crash).
_original_call = network.call
_crash_triggered = {"done": False}


def crashing_call(service_name):
    if service_name == "http:shipment" and not _crash_triggered["done"]:
        _crash_triggered["done"] = True
        raise RuntimeError("СИМУЛЬОВАНИЙ КРАХ ПРОЦЕСУ (наприклад, вимкнули сервер)")
    return _original_call(service_name)


network.call = crashing_call
network.call_log.clear()

try:
    call_user_function("processOrder", ["order-777"])
    print("ПОМИЛКА ТЕСТУ: крах мав статись, але не стався")
except RuntimeError as e:
    print(f"Процес 'впав' як і очікувалось: {e}")

print(f"Мережевих викликів до краху: {network.call_log}")

print()
print("=" * 70)
print("СИМУЛЯЦІЯ 2: 'перезапускаємо' виконання processOrder з тим самим orderId")
print("Очікується: fetchOrder і chargeCard НЕ виконуються повторно (з журналу),")
print("            createShipment виконується вперше (бо раніше не завершився)")
print("=" * 70)

network.call = _original_call  # прибираємо симуляцію краху - сервіс тепер working
network.call_log.clear()

result = call_user_function("processOrder", ["order-777"])
print(f"Результат другого запуску: {result!r}")
print(f"Мережевих викликів у другому запуску: {network.call_log}")

only_shipment = all("shipment" in c for c in network.call_log)
print(f">>> Жодного повторного виклику orders/charge (тільки shipment): "
      f"{'ТАК' if only_shipment and len(network.call_log) > 0 else 'ПРОВАЛ'}")
print(f">>> Результат 'done' отримано попри крах посередині: "
      f"{'ТАК' if result == 'done' else 'ПРОВАЛ'}")
