"""
Oberih Interpreter v0.1 (Фаза 1: tree-walking)

Реалізує мінімальне ядро мови:
- resilient fn з модифікаторами: deadline, retryBudget, retries, fallback
- Shared Budget: спільний бюджет спроб передається неявно через вкладені виклики
- Result-подібна поведінка через оператор "?": якщо виклик кидає Failure,
  помилка прокидається нагору автоматично
- ResilienceExhausted: коли бюджет вичерпано і fallback теж не рятує

Мета цього прототипу — довести, що Shared Budget реально обмежує сумарну
кількість спроб у ланцюжку викликів, а не є лише ідеєю на папері.
"""

from lark import Lark, Transformer
import contextvars
import time
import concurrent.futures
import threading


# ---------------------------------------------------------------------------
# Парсер
# ---------------------------------------------------------------------------

with open("grammar.lark") as f:
    GRAMMAR = f.read()

parser = Lark(GRAMMAR, parser="lalr", propagate_positions=True)


# ---------------------------------------------------------------------------
# Рантайм-примітиви
# ---------------------------------------------------------------------------

class ResilienceExhausted(Exception):
    """Кидається, коли Shared Budget вичерпано і fallback теж провалився."""
    def __init__(self, fn_name):
        super().__init__(f"Resilience budget exhausted in '{fn_name}'")
        self.fn_name = fn_name


class NetworkFailure(Exception):
    """Симуляція мережевої помилки (наприклад http.get впав)."""
    pass


class TimeoutFailure(NetworkFailure):
    """Спроба перевищила відведений timeout."""
    def __init__(self, fn_name, seconds):
        super().__init__(f"'{fn_name}' перевищив timeout {seconds}s")


# Виконуємо потенційно "довгі" виклики в окремому потоці, щоб мати змогу
# примусово припинити ЧЕКАННЯ на результат після timeout (сам потік з мок-
# затримкою може ще довиконатись у фоні - для реального мережевого I/O це
# природно відповідає тому, як cancel працює у більшості мов).
_timeout_executor = concurrent.futures.ThreadPoolExecutor(max_workers=16)


class RateLimitedError(NetworkFailure):
    def __init__(self, fn_name):
        super().__init__(f"'{fn_name}' перевищив rateLimit - запит відхилено")


class BulkheadRejectedError(NetworkFailure):
    def __init__(self, fn_name):
        super().__init__(f"'{fn_name}' - bulkhead заповнений, забагато одночасних викликів")


_rate_limit_state = {}      # fn_name -> [timestamps у вікні]
_bulkhead_semaphores = {}   # fn_name -> threading.Semaphore
_bulkhead_lock = threading.Lock()


def _rate_limit_allow(fn_name, cfg):
    now = time.monotonic()
    timestamps = _rate_limit_state.setdefault(fn_name, [])
    while timestamps and now - timestamps[0] > cfg["per"]:
        timestamps.pop(0)
    if len(timestamps) >= cfg["n"]:
        return False
    timestamps.append(now)
    return True


def _get_bulkhead_semaphore(fn_name, max_concurrent):
    with _bulkhead_lock:
        if fn_name not in _bulkhead_semaphores:
            _bulkhead_semaphores[fn_name] = threading.Semaphore(max_concurrent)
        return _bulkhead_semaphores[fn_name]


def _invoke_with_hedging(body_fn, call_args, after_seconds):
    """Запускає другу паралельну спробу, якщо перша не встигла за 'after'.
    Повертає результат тієї спроби, що завершилась успішно першою."""
    fut1 = _timeout_executor.submit(body_fn, *call_args)
    try:
        return fut1.result(timeout=after_seconds)
    except concurrent.futures.TimeoutError:
        pass  # перша спроба забарилась - запускаємо дублюючу
    except NetworkFailure:
        raise  # перша спроба вже провалилась швидко - hedging тут не допоможе

    fut2 = _timeout_executor.submit(body_fn, *call_args)
    pending = [fut1, fut2]
    last_exc = None
    while pending:
        done, pending = concurrent.futures.wait(pending, return_when=concurrent.futures.FIRST_COMPLETED)
        pending = list(pending)
        for f in done:
            try:
                return f.result()
            except Exception as e:
                last_exc = e
    raise last_exc


# Контекст спільного бюджету, що неявно передається через вкладені виклики.
# Це і є технічна реалізація Shared Budget з нашої специфікації.
_current_budget = contextvars.ContextVar("oberih_budget", default=None)


class Budget:
    def __init__(self, deadline_seconds, retry_budget, owner_fn):
        self.deadline_at = time.monotonic() + deadline_seconds
        self.retries_left = retry_budget
        self.owner_fn = owner_fn
        self.attempts_log = []  # для демонстрації/дебагу

    def is_expired(self):
        return time.monotonic() > self.deadline_at or self.retries_left <= 0

    def consume_attempt(self, fn_name):
        self.retries_left -= 1
        self.attempts_log.append(fn_name)


# ---------------------------------------------------------------------------
# Мокова мережа — щоб продемонструвати retry storm prevention без реального I/O
# ---------------------------------------------------------------------------

class MockNetwork:
    """Дозволяє налаштувати: цей сервіс падає N разів, потім відповідає ОК."""
    def __init__(self):
        self.fail_counts = {}   # service_name -> скільки ще разів впаде
        self.delays = {}        # service_name -> штучна затримка в секундах
        self.call_log = []      # хронологія всіх реальних викликів

    def configure_failures(self, service_name, times):
        self.fail_counts[service_name] = times

    def configure_delay(self, service_name, seconds):
        self.delays[service_name] = seconds

    def call(self, service_name):
        self.call_log.append(service_name)
        delay = self.delays.get(service_name, 0)
        if delay:
            time.sleep(delay)
        remaining = self.fail_counts.get(service_name, 0)
        if remaining > 0:
            self.fail_counts[service_name] = remaining - 1
            raise NetworkFailure(f"{service_name} failed (network error)")
        payload = {"service": service_name, "status": "ok", "id": "mock-id-123"}
        if "orders" in service_name:
            payload["userId"] = "user-42"
        return payload


network = MockNetwork()


# Стан circuit breaker persists між викликами (на відміну від Budget,
# який живе тільки в межах одного ланцюжка викликів).
_circuit_breakers = {}      # fn_name -> {"failures", "state", "opened_at"}
_idempotency_cache = {}     # (fn_name, key) -> результат успішного виклику
_result_cache = {}          # (fn_name, args) -> (expires_at, результат)


class CircuitOpenError(NetworkFailure):
    def __init__(self, fn_name):
        super().__init__(f"Circuit breaker OPEN for '{fn_name}' - виклик заблоковано")


def _breaker_is_open(fn_name, cfg):
    state = _circuit_breakers.setdefault(
        fn_name, {"failures": 0, "state": "closed", "opened_at": 0}
    )
    if state["state"] == "open":
        if time.monotonic() - state["opened_at"] >= cfg["cooldown"]:
            state["state"] = "half-open"  # даємо один пробний шанс
            return False
        return True
    return False


def _breaker_record_success(fn_name):
    state = _circuit_breakers.get(fn_name)
    if state:
        state["failures"] = 0
        state["state"] = "closed"


def _breaker_record_failure(fn_name, cfg):
    state = _circuit_breakers.setdefault(
        fn_name, {"failures": 0, "state": "closed", "opened_at": 0}
    )
    state["failures"] += 1
    if state["failures"] >= cfg["failThreshold"]:
        state["state"] = "open"
        state["opened_at"] = time.monotonic()


def circuit_breaker_snapshot():
    """Для дебагу/демонстрації - поточний стан усіх breaker'ів."""
    return {k: dict(v) for k, v in _circuit_breakers.items()}


# ---------------------------------------------------------------------------
# Виконання resilient-функції зі Shared Budget
# ---------------------------------------------------------------------------

def call_resilient(fn_name, modifiers, body_fn, call_args=()):
    """
    Обгортає виклик тіла функції логікою стійкості.

    modifiers: dict з розпарсеними модифікаторами цієї функції
    body_fn: python-функція, що представляє тіло resilient fn
    call_args: реальні аргументи виклику - потрібні для ключа кешування
    """
    # --- Cache: перша лінія захисту, взагалі уникає мережевого виклику ---
    cache_key = None
    if "cache" in modifiers:
        cache_key = (fn_name, tuple(repr(a) for a in call_args))
        cached = _result_cache.get(cache_key)
        if cached is not None and time.monotonic() < cached[0]:
            return cached[1]

    # --- Idempotency: якщо цей ключ вже успішно оброблено - не виконувати знову ---
    idem_key = None
    if "idempotent" in modifiers:
        idem_key = (fn_name, repr(modifiers["idempotent"]["key"]))
        if idem_key in _idempotency_cache:
            return _idempotency_cache[idem_key]

    parent_budget = _current_budget.get()

    if "deadline" in modifiers or "retryBudget" in modifiers:
        budget = Budget(
            deadline_seconds=modifiers.get("deadline", 9999),
            retry_budget=modifiers.get("retryBudget", 9999),
            owner_fn=fn_name,
        )
    else:
        budget = parent_budget
        if budget is None:
            raise RuntimeError(
                f"'{fn_name}' немає ні власного deadline/retryBudget, "
                f"ні батьківського бюджету. Компілятор мав би це відхилити "
                f"на кореневому виклику."
            )

    token = _current_budget.set(budget)
    try:
        has_explicit_retries = "retries" in modifiers
        local_retries = modifiers.get("retries", 1)
        breaker_cfg = modifiers.get("circuitBreaker")
        rate_cfg = modifiers.get("rateLimit")
        bulkhead_cfg = modifiers.get("bulkhead")
        hedging_cfg = modifiers.get("hedging")
        last_error = None

        attempt = 0
        while attempt < local_retries:
            if budget.is_expired():
                break

            if breaker_cfg and _breaker_is_open(fn_name, breaker_cfg):
                last_error = CircuitOpenError(fn_name)
                break  # блокуємо виклик - жодного реального навантаження на впалий сервіс

            if rate_cfg and not _rate_limit_allow(fn_name, rate_cfg):
                last_error = RateLimitedError(fn_name)
                break  # ліміт частоти вичерпано - не робимо запит взагалі

            sem = None
            if bulkhead_cfg:
                sem = _get_bulkhead_semaphore(fn_name, bulkhead_cfg["maxConcurrent"])
                if not sem.acquire(blocking=False):
                    last_error = BulkheadRejectedError(fn_name)
                    break  # усі "місця" зайняті - відхиляємо, не чекаючи в черзі

            if has_explicit_retries:
                budget.consume_attempt(fn_name)
            attempt += 1
            try:
                if hedging_cfg:
                    result = _invoke_with_hedging(body_fn, call_args, hedging_cfg["after"])
                elif "timeout" in modifiers:
                    future = _timeout_executor.submit(body_fn, *call_args)
                    try:
                        result = future.result(timeout=modifiers["timeout"])
                    except concurrent.futures.TimeoutError:
                        raise TimeoutFailure(fn_name, modifiers["timeout"]) from None
                else:
                    result = body_fn(*call_args)

                if breaker_cfg:
                    _breaker_record_success(fn_name)
                if cache_key is not None:
                    _result_cache[cache_key] = (
                        time.monotonic() + modifiers["cache"]["ttl"],
                        result,
                    )
                if idem_key is not None:
                    _idempotency_cache[idem_key] = result
                return result
            except NetworkFailure as e:
                last_error = e
                if breaker_cfg:
                    _breaker_record_failure(fn_name, breaker_cfg)
                continue
            finally:
                if sem is not None:
                    sem.release()

        # --- Вичерпано: fallback -> emergencyFallback -> ResilienceExhausted ---
        fallback_defined = "fallback" in modifiers
        emergency_defined = "emergency_fallback" in modifiers

        if fallback_defined:
            try:
                return modifiers["fallback"]()
            except Exception:
                pass  # провал fallback - падаємо нижче до emergency/exhausted

        if emergency_defined:
            try:
                return modifiers["emergency_fallback"]()
            except Exception:
                raise ResilienceExhausted(fn_name) from last_error

        if fallback_defined:
            # fallback був, але теж провалився, emergency відсутній
            raise ResilienceExhausted(fn_name) from last_error

        if last_error is not None:
            raise last_error
        raise ResilienceExhausted(fn_name)
    finally:
        _current_budget.reset(token)
