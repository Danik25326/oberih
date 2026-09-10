"""
Демонстрація: чи справді Shared Budget запобігає retry storm?

Сценарій: fetchOrder, fetchUser, fetchPaymentStatus - кожна оголошує
власний retries(3). Якщо композиція була б "незалежною" (варіант A,
який ми відхилили) - максимум спроб = 3+3+3 = 9 реальних мережевих
викликів. Ми обрали Shared Budget: сумарно не більше retryBudget(6),
незалежно від того, що просить кожна функція окремо.

Тут ми навмисно змушуємо ВСІ три сервіси постійно падати (нескінченно),
щоб побачити граничну поведінку: скільки реальних мережевих викликів
буде зроблено, перш ніж спрацює fallback.
"""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

from interpreter import call_resilient, network, ResilienceExhausted


def fetch_order(order_id):
    def body(order_id):
        return network.call("orders-service")
    return call_resilient(
        "fetchOrder",
        {"retries": 3},
        body,
        (order_id,),
    )


def fetch_user(user_id):
    def body(user_id):
        return network.call("users-service")
    return call_resilient(
        "fetchUser",
        {"retries": 3},
        body,
        (user_id,),
    )


def fetch_payment_status(order_id):
    def body(order_id):
        return network.call("payments-service")
    return call_resilient(
        "fetchPaymentStatus",
        {"retries": 2},
        body,
        (order_id,),
    )


def cached_summary(order_id):
    return {"source": "cache", "order_id": order_id, "degraded": True}


def get_order_summary(order_id):
    def body(order_id):
        order = fetch_order(order_id)
        user = fetch_user("user-42")
        payment = fetch_payment_status(order_id)
        return {"order": order, "user": user, "payment": payment}

    return call_resilient(
        "getOrderSummary",
        {
            "deadline": 2.0,        # секунди
            "retryBudget": 6,       # СПІЛЬНИЙ бюджет на весь ланцюжок
            "fallback": lambda: cached_summary(order_id),
        },
        body,
        (order_id,),
    )


if __name__ == "__main__":
    print("=" * 70)
    print("ТЕСТ 1: усі три сервіси постійно падають (нескінченно)")
    print("Без Shared Budget очікувалось би до 3+3+2 = 8 реальних викликів.")
    print("=" * 70)

    network.configure_failures("orders-service", 999)
    network.configure_failures("users-service", 999)
    network.configure_failures("payments-service", 999)
    network.call_log.clear()

    result = get_order_summary("order-123")

    print(f"\nРезультат: {result}")
    print(f"Реальних мережевих викликів зроблено: {len(network.call_log)}")
    print(f"Хронологія викликів: {network.call_log}")
    print(f"\n>>> Перевірка: {len(network.call_log)} <= 6 (retryBudget)? "
          f"{'ТАК, budget спрацював' if len(network.call_log) <= 6 else 'ПРОВАЛ дизайну'}")

    print()
    print("=" * 70)
    print("ТЕСТ 2: сервіси падають лише 1 раз, потім відповідають ОК")
    print("(типовий 'мережа моргнула' сценарій - все має вдатись)")
    print("=" * 70)

    network.configure_failures("orders-service", 1)
    network.configure_failures("users-service", 1)
    network.configure_failures("payments-service", 1)
    network.call_log.clear()

    result = get_order_summary("order-456")

    print(f"\nРезультат: {result}")
    print(f"Реальних мережевих викликів зроблено: {len(network.call_log)}")
    print(f"Хронологія викликів: {network.call_log}")
    print(f"\n>>> Дані отримані успішно, без fallback: "
          f"{'ТАК' if result.get('order') else 'НІ, ЩОСЬ ЗЛАМАЛОСЬ'}")
