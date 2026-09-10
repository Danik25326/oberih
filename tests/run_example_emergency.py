import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from typechecker import check_program, type_check_program
from interpreter import network

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn riskyCall(x: String) -> Result<String, Error>
    deadline(2s)
    retryBudget(2)
    fallback(x.brokenField)
    emergencyFallback("guest-default")
{
    return http.get("/risky/" + x)
}
"""

tree = parser.parse(source)
load_program(tree)
print("Помилки компіляції:", check_program(evaluator.REGISTRY) + type_check_program(evaluator.REGISTRY) or "немає")

# cachedFallback - невідома функція, викличе виключення при спробі виконати ->
# симулює ситуацію "навіть fallback провалився"
network.configure_failures("http:risky", 999)
network.call_log.clear()

result = call_user_function("riskyCall", ["item-1"])
print(f"Результат: {result!r}")
print(f">>> emergencyFallback спрацював, коли впав і основний fallback: "
      f"{'ТАК' if result == 'guest-default' else 'ПРОВАЛ'}")
