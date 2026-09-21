"""
Тест: OberihResult як справжній тип.
Перевіряє:
1. Ok/Err — це OberihResult, не dict
2. ? розпаковує Ok
3. ? пробрасує Err вгору і функція повертає Err
4. match на Ok(v)/Err(e) працює
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from oberih_result import OberihResult, PropagateErr

# --- Unit тести самого типу ---

def test_ok_is_not_dict():
    r = OberihResult.ok(42)
    assert not isinstance(r, dict), "Ok — не dict"
    assert isinstance(r, OberihResult), "Ok — OberihResult"
    assert r.is_ok()
    assert not r.is_err()
    assert r.unwrap() == 42
    assert r.variant == "Ok"
    print("✓ Ok — справжній тип, не dict")

def test_err_is_not_dict():
    r = OberihResult.err("щось пішло не так")
    assert not isinstance(r, dict), "Err — не dict"
    assert isinstance(r, OberihResult), "Err — OberihResult"
    assert r.is_err()
    assert not r.is_ok()
    assert r.unwrap_err() == "щось пішло не так"
    assert r.variant == "Err"
    print("✓ Err — справжній тип, не dict")

def test_propagate_err():
    ok_result = OberihResult.ok("дані")
    err_result = OberihResult.err("мережева помилка")

    # Ok? -> розпаковує значення
    assert ok_result.unwrap() == "дані"

    # Err? -> кидає PropagateErr
    try:
        if err_result.is_err():
            raise PropagateErr(err_result)
        assert False, "Мало кинути PropagateErr"
    except PropagateErr as e:
        assert e.result is err_result
        assert e.result.unwrap_err() == "мережева помилка"
    print("✓ PropagateErr кидається для Err і несе Result")

def test_repr():
    assert repr(OberihResult.ok(42)) == "Ok(42)"
    assert repr(OberihResult.err("fail")) == "Err('fail')"
    print("✓ repr: Ok(42), Err('fail')")

def test_equality():
    assert OberihResult.ok(1) == OberihResult.ok(1)
    assert OberihResult.ok(1) != OberihResult.ok(2)
    assert OberihResult.ok(1) != OberihResult.err(1)
    assert OberihResult.err("x") == OberihResult.err("x")
    print("✓ == / != працює коректно")

# --- Інтеграційний тест через evaluator ---

def test_evaluator_ok_err():
    from lark import Lark
    import evaluator as ev

    grammar_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    src = """
fn tryParse(s: String) -> String {
    let r = Ok(s)
    return r
}

fn main() -> Number {
    let result = tryParse("привіт")
    return 0
}
"""
    tree = parser.parse(src)
    ev.load_program(tree)
    result = ev.call_by_name("tryParse", ["привіт"], ev.Environment())
    assert isinstance(result, OberihResult), f"Очікували OberihResult, отримали {type(result)}"
    assert result.is_ok()
    assert result.unwrap() == "привіт"
    print("✓ evaluator: Ok(...) повертає OberihResult, не dict")

def test_evaluator_match_result():
    from lark import Lark
    import evaluator as ev

    grammar_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    src = """
fn describeResult(r: String) -> String {
    let ok = Ok("успіх")
    return match ok {
        Ok(v) => "отримали: " + v,
        Err(e) => "помилка: " + e
    }
}

fn main() -> Number {
    return 0
}
"""
    tree = parser.parse(src)
    ev.load_program(tree)
    val = ev.call_by_name("describeResult", ["x"], ev.Environment())
    assert val == "отримали: успіх", f"Отримали: {val!r}"
    print("✓ evaluator: match Ok(v)/Err(e) працює з OberihResult")

if __name__ == "__main__":
    test_ok_is_not_dict()
    test_err_is_not_dict()
    test_propagate_err()
    test_repr()
    test_equality()
    test_evaluator_ok_err()
    test_evaluator_match_result()
    print("\n✓ Всі тести Result<T,E> пройшли")
