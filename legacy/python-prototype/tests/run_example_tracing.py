import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import network, get_trace_spans, clear_trace_spans, print_trace_tree

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn fetchOrder(id: String) -> Result<Order, Error>
    retries(2)
{
    return http.get("/orders/" + id)
}

resilient fn fetchUser(id: String) -> Result<User, Error>
    retries(2)
{
    return http.get("/users/" + id)
}

resilient fn getOrderSummary(orderId: String) -> Result<String, Error>
    deadline(3s)
    retryBudget(5)
    traced
{
    let order = fetchOrder(orderId)?
    let user = fetchUser(order.userId)?
    return "ok"
}
"""

tree = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ТЕСТ: тільки getOrderSummary позначена 'traced'.")
print("Очікується: fetchOrder і fetchUser АВТОМАТИЧНО отримали спани,")
print("            хоча жодна з них не позначена 'traced' сама по собі.")
print("=" * 70)

clear_trace_spans()
network.call_log.clear()
result = call_user_function("getOrderSummary", ["order-1"])
print(f"Результат: {result}")
print()
print("Дерево спанів (згенеровано АВТОМАТИЧНО, без жодного рядка ручного коду):")
print_trace_tree()

spans = get_trace_spans()
fn_names = {s["fn_name"] for s in spans}
print()
print(f"Усього спанів: {len(spans)}, функції: {fn_names}")
print(f">>> fetchOrder і fetchUser отримали спани автоматично: "
      f"{'ТАК' if 'fetchOrder' in fn_names and 'fetchUser' in fn_names else 'ПРОВАЛ'}")
print(f">>> Усі спани success: "
      f"{'ТАК' if all(s['outcome'] == 'success' for s in spans) else 'ПРОВАЛ'}")
