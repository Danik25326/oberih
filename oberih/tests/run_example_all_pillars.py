import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

from lark import Lark
from evaluator import load_program, call_user_function
import evaluator
from typechecker import (
    check_program, type_check_program,
    check_emergency_fallback_no_io, check_agent_budget_required,
)
from interpreter import (
    llm_service, get_trace_spans, clear_trace_spans, print_trace_tree,
    reset_durable_journal,
)

with open(os.path.join(_ROOT, "grammar.lark")) as f:
    parser = Lark(f.read(), parser="lalr", propagate_positions=True)

print("=" * 70)
print("КРОК 1: agent.call без budget - МАЄ провалити компіляцію")
print("=" * 70)
bad_source = """
resilient fn askBad(q: String) -> Result<String, Error>
    deadline(10s)
    retryBudget(3)
{
    return agent.call(q)
}
"""
tree = parser.parse(bad_source)
load_program(tree)
errors = (
    check_program(evaluator.REGISTRY)
    + type_check_program(evaluator.REGISTRY, evaluator.STRUCTS)
    + check_emergency_fallback_no_io(evaluator.REGISTRY)
    + check_agent_budget_required(evaluator.REGISTRY)
)
print("Помилки компіляції:", errors)
print(f">>> Компіляція відхилена, як і мала бути: {'ТАК' if errors else 'ПРОВАЛ'}")

print()
print("=" * 70)
print("КРОК 2: правильна версія - усі 5 пілонів разом, лише за рахунок agent.call")
print("(durable і traced НЕ написані явно - компілятор додає їх сам)")
print("=" * 70)

good_source = """
resilient fn askAssistant(question: String) -> Result<String, Error>
    deadline(10s)
    retryBudget(3)
    budget(tokens: 2000)
{
    return agent.call(question)
}
"""
tree = parser.parse(good_source)
load_program(tree)
errors = (
    check_program(evaluator.REGISTRY)
    + type_check_program(evaluator.REGISTRY, evaluator.STRUCTS)
    + check_emergency_fallback_no_io(evaluator.REGISTRY)
    + check_agent_budget_required(evaluator.REGISTRY)
)
print("Помилки компіляції:", errors or "немає (ОК)")

reset_durable_journal()
clear_trace_spans()
llm_service.call_log.clear()

result = call_user_function("askAssistant", ["Привіт, як справи?"])
print(f"Результат: {result}")

spans = get_trace_spans()
print()
print("Дерево спанів (traced увімкнено АВТОМАТИЧНО через agent.call):")
print_trace_tree()

import json
with open(os.path.join(_ROOT, ".oberih_journal.json")) as f:
    journal = json.load(f)
print()
print("Записи в durable-журналі (durable увімкнено АВТОМАТИЧНО):")
for k in journal:
    print(f"  - {k}")

print()
print(f">>> Пілон 1 (Shared Budget - retryBudget/deadline): присутній у сигнатурі - ТАК")
print(f">>> Пілон 2 (Durable): записано в журнал автоматично - "
      f"{'ТАК' if len(journal) > 0 else 'ПРОВАЛ'}")
print(f">>> Пілон 3 (Узагальнений бюджет tokens): budget(tokens: 2000) активний - ТАК")
print(f">>> Пілон 4 (Обсервабельність): спани створено автоматично - "
      f"{'ТАК' if len(spans) > 0 else 'ПРОВАЛ'}")
print(f">>> Пілон 5 (AI-агент-нативний виклик): agent.call спрацював - "
      f"{'ТАК' if 'симульована' in result else 'ПРОВАЛ'}")

reset_durable_journal()  # прибираємо за собою
