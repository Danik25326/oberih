"""
Тест: OberihStruct як справжній reference тип.
Перевіряє:
1. Struct — OberihStruct, не dict
2. get_field / set_field працюють
3. Мутація через self в методі видна зовні (reference semantics)
4. copy() дає незалежну копію
5. repr людяний
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from oberih_struct import OberihStruct

def test_not_dict():
    s = OberihStruct("Point", {"x": 3, "y": 4})
    assert not isinstance(s, dict), "Struct — не dict"
    assert isinstance(s, OberihStruct)
    assert s.get_field("x") == 3
    assert s.get_field("y") == 4
    print("✓ OberihStruct — не dict")

def test_mutation():
    s = OberihStruct("Point", {"x": 3, "y": 4})
    s.set_field("x", 10)
    assert s.get_field("x") == 10
    print("✓ set_field мутує значення")

def test_reference_semantics():
    """Мутація через аліас видна в оригіналі — reference semantics."""
    s = OberihStruct("Point", {"x": 3, "y": 4})
    alias = s  # не копія — та ж референція
    alias.set_field("x", 99)
    assert s.get_field("x") == 99, "Мутація через аліас має бути видна"
    print("✓ Reference semantics: мутація через аліас видна в оригіналі")

def test_copy_is_independent():
    s = OberihStruct("Point", {"x": 3, "y": 4})
    c = s.copy()
    c.set_field("x", 99)
    assert s.get_field("x") == 3, "copy() — незалежна"
    assert c.get_field("x") == 99
    print("✓ copy() дає незалежну структуру")

def test_repr():
    s = OberihStruct("Point", {"x": 3, "y": 4})
    assert repr(s) == "Point{x: 3, y: 4}"
    print(f"✓ repr: {repr(s)}")

def test_equality():
    a = OberihStruct("Point", {"x": 3, "y": 4})
    b = OberihStruct("Point", {"x": 3, "y": 4})
    c = OberihStruct("Point", {"x": 0, "y": 0})
    assert a == b
    assert a != c
    print("✓ == / != за значенням полів")

def test_invalid_field():
    s = OberihStruct("Point", {"x": 3, "y": 4})
    try:
        s.get_field("z")
        assert False, "Мало кинути AttributeError"
    except AttributeError as e:
        assert "z" in str(e)
    print("✓ get_field на неіснуючому полі кидає AttributeError")

# --- Інтеграційні тести через evaluator ---

def test_evaluator_struct_not_dict():
    from lark import Lark
    import evaluator as ev

    grammar_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    src = """
struct Point {
    x: Number,
    y: Number
}

fn makePoint(a: Number, b: Number) -> Point {
    return Point(a, b)
}

fn main() -> Number {
    return 0
}
"""
    tree = parser.parse(src)
    ev.load_program(tree)
    result = ev.call_by_name("makePoint", [3, 4], ev.Environment())
    assert isinstance(result, OberihStruct), f"Очікували OberihStruct, отримали {type(result)}"
    assert not isinstance(result, dict)
    assert result.get_field("x") == 3
    assert result.get_field("y") == 4
    print("✓ evaluator: Point(3,4) повертає OberihStruct, не dict")

def test_evaluator_self_mutation_visible():
    """self.x = ... в методі має бути видно зовні через reference."""
    from lark import Lark
    import evaluator as ev

    grammar_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    src = """
struct Counter {
    value: Number
}

fn Counter.increment(self: Counter) -> Number {
    self.value = self.value + 1
    return 0
}

fn main() -> Number {
    let c = Counter(0)
    c.increment()
    c.increment()
    c.increment()
    return c.value
}
"""
    tree = parser.parse(src)
    ev.load_program(tree)
    result = ev.call_by_name("main", [], ev.Environment())
    assert result == 3, f"Очікували 3 після 3 increment(), отримали {result}"
    print("✓ evaluator: self мутація видна зовні (reference semantics)")

if __name__ == "__main__":
    test_not_dict()
    test_mutation()
    test_reference_semantics()
    test_copy_is_independent()
    test_repr()
    test_equality()
    test_invalid_field()
    test_evaluator_struct_not_dict()
    test_evaluator_self_mutation_visible()
    print("\n✓ Всі тести OberihStruct пройшли")
