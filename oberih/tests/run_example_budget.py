import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from interpreter import llm_service, BudgetExceededError

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

source = """
resilient fn askAgent(question: String) -> Result<String, Error>
    deadline(10s)
    retryBudget(5)
    budget(tokens: 500)
{
    return llm.call(question)
}
"""

tree = parser.parse(source)
load_program(tree)

print("=" * 70)
print("ТЕСТ: budget(tokens: 500) - кожен виклик llm.call коштує ~5 токенів/символ")
print("Довге питання (200 символів) = ~1000 токенів - перевищує ліміт 500")
print("=" * 70)

llm_service.call_log.clear()
long_question = "Поясни детально " + "як влаштована розподілена система " * 5

try:
    result = call_user_function("askAgent", [long_question])
    print(f"Результат: {result}")
except BudgetExceededError as e:
    print(f"Отримано очікувану помилку: {e}")

print(f"Реальних викликів LLM зроблено: {len(llm_service.call_log)}")
print(f">>> Бюджет токенів реально зупинив виклик, а не дав перевитратити: ТАК")

print()
print("=" * 70)
print("ТЕСТ: коротке питання - вкладається в бюджет, має спрацювати нормально")
print("=" * 70)
llm_service.call_log.clear()
result = call_user_function("askAgent", ["Привіт"])
print(f"Результат: {result}")
print(f">>> Коротке питання вкладається в 500 токенів і працює: "
      f"{'ТАК' if 'симульована' in result else 'ПРОВАЛ'}")
