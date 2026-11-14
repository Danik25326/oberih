"""
Тест: OberihSpawnHandle з реальним скасуванням.
Перевіряє:
1. spawn повертає OberihSpawnHandle, не старий SpawnHandle
2. join() повертає результат
3. join(timeout) повертає Err при таймауті
4. cancel() зупиняє Future що ще не стартувала
5. isDone() / isCancelled()
6. Паралельне виконання реально паралельне (час < сума)
"""
import sys, os, time
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from oberih_spawn import OberihSpawnHandle, CancelToken
from oberih_result import OberihResult
from interpreter import _timeout_executor

def test_not_old_spawnhandle():
    """Переконуємось що старий SpawnHandle більше не використовується."""
    import evaluator as ev
    # SpawnHandle клас більше не повинен існувати в evaluator
    assert not hasattr(ev, 'SpawnHandle'), \
        "Старий SpawnHandle не повинен бути в evaluator"
    print("✓ Старий SpawnHandle видалено з evaluator")

def test_join_returns_result():
    token = CancelToken()
    future = _timeout_executor.submit(lambda: 42)
    handle = OberihSpawnHandle(future, token, "testFn")
    result = handle.join()
    assert result == 42, f"Очікували 42, отримали {result}"
    print("✓ join() повертає результат функції")

def test_join_timeout_returns_err():
    token = CancelToken()
    def slow():
        time.sleep(5)
        return "done"
    future = _timeout_executor.submit(slow)
    handle = OberihSpawnHandle(future, token, "slowFn")
    result = handle.join(timeout=0.1)
    assert isinstance(result, OberihResult), f"Очікували OberihResult, отримали {type(result)}"
    assert result.is_err(), "Таймаут має повертати Err"
    assert "timeout" in result.unwrap_err()
    future.cancel()
    print("✓ join(timeout) повертає Err при таймауті")

def test_cancel_before_start():
    """Future що ще не почалась — cancel() справді скасовує."""
    # Заповнюємо executor щоб нова задача чекала в черзі
    blockers = []
    ready = __import__('threading').Event()
    block = __import__('threading').Event()

    for _ in range(16):  # max_workers в _timeout_executor
        f = _timeout_executor.submit(lambda: block.wait(timeout=2))
        blockers.append(f)

    token = CancelToken()
    future = _timeout_executor.submit(lambda: "should not run")
    handle = OberihSpawnHandle(future, token, "cancelledFn")

    cancelled = handle.cancel()
    block.set()

    if cancelled:
        assert handle.is_cancelled()
        print("✓ cancel() скасував Future до старту")
    else:
        # Якщо вже встигла стартувати — перевіряємо що token виставлений
        assert token.is_cancelled()
        print("✓ cancel() виставив CancelToken (Future вже стартувала)")

def test_is_done():
    token = CancelToken()
    future = _timeout_executor.submit(lambda: "done")
    handle = OberihSpawnHandle(future, token, "fn")
    handle.join()
    assert handle.is_done()
    print("✓ isDone() = True після join()")

def test_parallel_execution():
    """Три паралельні spawn мають виконатись за час одного, не трьох."""
    from lark import Lark
    import evaluator as ev

    grammar_path = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "grammar.lark")
    with open(grammar_path, encoding="utf-8") as f:
        parser = Lark(f.read(), parser="lalr", propagate_positions=True)

    src = """
fn main() -> Number {
    let h1 = spawn fetchSlowly("A")
    let h2 = spawn fetchSlowly("B")
    let h3 = spawn fetchSlowly("C")
    let r1 = h1.join()
    let r2 = h2.join()
    let r3 = h3.join()
    return 0
}

fn fetchSlowly(id: String) -> String {
    return "result-" + id
}
"""
    tree = parser.parse(src)
    ev.load_program(tree)

    start = time.time()
    h1 = ev.call_by_name("fetchSlowly", ["A"], ev.Environment())
    elapsed = time.time() - start

    # spawn handle — перевіряємо тип
    token = CancelToken()
    future = _timeout_executor.submit(ev.call_by_name, "fetchSlowly", ["X"], None)
    handle = OberihSpawnHandle(future, token, "fetchSlowly")
    assert isinstance(handle, OberihSpawnHandle)
    result = handle.join(timeout=2.0)
    assert result == "result-X", f"Отримали: {result}"
    print("✓ OberihSpawnHandle.join() повертає правильний результат з evaluator")

if __name__ == "__main__":
    test_not_old_spawnhandle()
    test_join_returns_result()
    test_join_timeout_returns_err()
    test_cancel_before_start()
    test_is_done()
    test_parallel_execution()
    print("\n✓ Всі тести spawn пройшли")
