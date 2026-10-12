"""
`oberih.py explain <файл.obh> [функція]` - показує ДО запуску, як Shared
Budget розподіляється по дереву вкладених resilient-викликів для заданої
функції: хто встановлює власний deadline/retryBudget, а хто успадковує
від батька.

Це не існує в жодній іншій мові - природне продовження ідентичності
Oberih, де сам розподіл бюджету є першокласною концепцією мови.
"""

from evaluator import Environment, build_modifiers
from typechecker import _walk_calls

_BUDGET_KEYS = ("deadline", "retryBudget", "retries", "budget")


def _summarize_modifiers(modifier_nodes):
    """Намагається статично обчислити прості константні модифікатори
    (deadline/retryBudget/retries/budget). Якщо значення залежить від
    параметра функції (динамічне) - позначає це чесно, а не падає."""
    try:
        mods = build_modifiers(modifier_nodes, Environment())
    except Exception:
        return {"_dynamic": True}

    summary = {}
    for key in _BUDGET_KEYS:
        if key in mods:
            summary[key] = mods[key]
    if "fallback" in mods:
        summary["fallback"] = "(є)"
    if "emergency_fallback" in mods:
        summary["emergencyFallback"] = "(є)"
    return summary


def build_call_tree(fn_name, registry, visited=None):
    fn = registry.get(fn_name)
    if fn is None:
        return {"name": fn_name, "found": False, "modifiers": {}, "children": []}

    modifiers = _summarize_modifiers(fn["modifier_nodes"]) if fn["is_resilient"] else {}

    children_names = []
    if visited is None:
        visited = set()
    if fn_name not in visited:
        visited.add(fn_name)
        found = set()
        for stmt in fn["body"]:
            _walk_calls(stmt, registry, found)
        children_names = sorted(found)

    children = [build_call_tree(c, registry, visited) for c in children_names]

    return {
        "name": fn_name,
        "found": True,
        "is_resilient": fn["is_resilient"],
        "modifiers": modifiers,
        "children": children,
    }


def _format_modifiers(mods):
    if not mods:
        return "успадковує бюджет батька"
    if mods.get("_dynamic"):
        return "модифікатори залежать від аргументів (не обчислено статично)"
    has_own_budget = "deadline" in mods or "retryBudget" in mods
    parts = [f"{k}={v}" for k, v in mods.items() if k not in ("_dynamic",)]
    root_marker = "ВЛАСНИЙ БЮДЖЕТ" if has_own_budget else "успадковує бюджет батька"
    return f"{root_marker} · {', '.join(parts)}" if parts else root_marker


def print_tree(node, depth=0):
    indent = "  " * depth
    branch = "└─ " if depth > 0 else ""
    if not node["found"]:
        print(f"{indent}{branch}{node['name']}  (не знайдено - можливо, вбудована або з бібліотеки)")
        return

    kind = "resilient" if node["is_resilient"] else "звичайна"
    mods_str = _format_modifiers(node["modifiers"]) if node["is_resilient"] else "не потребує бюджету"
    print(f"{indent}{branch}{node['name']}  [{kind}]  {mods_str}")

    for child in node["children"]:
        print_tree(child, depth + 1)


# ---------------------------------------------------------------------------
# Прогноз найгіршого випадку (cost forecasting) - агрегує ВСІ незалежні
# Shared Budget, досяжні з даної функції, навіть через оркеструючі функції,
# що не є resilient самі, але викликають КІЛЬКА окремих resilient-ланцюжків.
# Кожен корінь Shared Budget вже сам по собі є верхньою межею для свого
# піддерева - тому далі вглиб не заходимо, а лише підсумовуємо корені.
# ---------------------------------------------------------------------------

def find_root_budgets(fn_name, registry, visited=None, roots=None):
    if visited is None:
        visited = set()
    if roots is None:
        roots = []
    if fn_name in visited:
        return roots
    visited.add(fn_name)

    fn = registry.get(fn_name)
    if fn is None:
        return roots

    if fn["is_resilient"]:
        mods = _summarize_modifiers(fn["modifier_nodes"])
        has_own_budget = "deadline" in mods or "retryBudget" in mods
        if has_own_budget:
            roots.append((fn_name, mods))
            return roots  # це піддерево вже покрито цим бюджетом - не йдемо глибше

    # Оркеструюча (не-resilient) функція чи resilient без власного бюджету -
    # шукаємо далі, бо тут можуть ховатись КІЛЬКА окремих Shared Budget.
    from typechecker import _walk_all_calls
    found = set()
    for stmt in fn["body"]:
        _walk_all_calls(stmt, registry, found)
    for child in sorted(found):
        find_root_budgets(child, registry, visited, roots)
    return roots


def print_forecast(fn_name, registry):
    roots = find_root_budgets(fn_name, registry)
    if not roots:
        print("Не знайдено жодного незалежного Shared Budget у дереві викликів.")
        return

    total_deadline = 0.0
    total_retries = 0
    total_tokens = 0.0
    total_cost = 0.0

    print(f"Прогноз найгіршого випадку для '{fn_name}' "
          f"(сума всіх незалежних Shared Budget у дереві):\n")

    for name, mods in roots:
        d = mods.get("deadline") or 0
        rb = mods.get("retryBudget") or 0
        budget = mods.get("budget") or {}
        tokens = budget.get("tokens") or 0
        cost = budget.get("cost") or 0
        total_deadline += d
        total_retries += rb
        total_tokens += tokens
        total_cost += cost
        extra = []
        if tokens:
            extra.append(f"tokens={tokens}")
        if cost:
            extra.append(f"cost={cost}")
        extra_str = (", " + ", ".join(extra)) if extra else ""
        print(f"  - {name}: deadline={d}s, retryBudget={rb}{extra_str}")

    print()
    print("РАЗОМ (верхня межа, у припущенні послідовного виконання):")
    print(f"  Максимальний час:        {total_deadline:.1f}s")
    print(f"  Максимум реальних спроб: {total_retries}")
    if total_tokens:
        print(f"  Максимум токенів:        {total_tokens:.0f}")
    if total_cost:
        print(f"  Максимальна вартість:    {total_cost:.4f}")
    print()
    print("Застереження: це верхня межа для послідовного виконання. Паралельні")
    print("виклики через spawn можуть завершитись швидше за сумарний час, але не")
    print("зменшують сумарну кількість спроб чи вартість.")


def run_explain(path, fn_name=None, forecast=False):
    from lark import Lark
    from module_loader import parse_with_imports, ObirihCompileError
    from evaluator import load_program, REGISTRY
    import os
    import sys

    grammar_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    try:
        tree = parse_with_imports(path, parser)
    except ObirihCompileError as e:
        print(str(e), file=sys.stderr)
        sys.exit(1)

    load_program(tree)

    if fn_name is None:
        resilient_names = [n for n, f in REGISTRY.items() if f["is_resilient"]]
        if not resilient_names:
            print("У файлі немає жодної resilient-функції.")
            return
        fn_name = resilient_names[0]
        print(f"(Функцію не вказано - показую першу знайдену resilient-функцію: '{fn_name}')\n")

    if fn_name not in REGISTRY:
        print(f"Функцію '{fn_name}' не знайдено у файлі.", file=sys.stderr)
        sys.exit(1)

    if forecast:
        print_forecast(fn_name, REGISTRY)
        return

    print(f"Дерево розподілу Shared Budget для '{fn_name}':\n")
    tree_data = build_call_tree(fn_name, REGISTRY)
    print_tree(tree_data)
