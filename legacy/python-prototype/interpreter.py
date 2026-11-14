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

import contextvars
import time
import concurrent.futures
import threading
import json
import os
import sys
import uuid
import urllib.request
import urllib.error
import datetime


# ---------------------------------------------------------------------------
# Рантайм-примітиви
# ---------------------------------------------------------------------------

class ResilienceExhausted(Exception):
    """Кидається, коли Shared Budget вичерпано і fallback теж провалився."""
    def __init__(self, fn_name):
        super().__init__(f"Resilience budget exhausted in '{fn_name}'")
        self.fn_name = fn_name


_INCIDENTS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".oberih_incidents")


def _write_incident_report(fn_name, budget, last_error):
    """Автоматичний post-mortem у момент ResilienceExhausted - усі дані вже
    зібрані під час виконання (бюджет, спроби, спани), просто зберігаємо їх
    структуровано на диск, замість того щоб вони губились у стеку викликів."""
    try:
        os.makedirs(_INCIDENTS_DIR, exist_ok=True)
        timestamp = datetime.datetime.now().isoformat()
        report = {
            "function": fn_name,
            "timestamp": timestamp,
            "attempts_made": list(budget.attempts_log) if budget else [],
            "retries_left_at_failure": budget.retries_left if budget else None,
            "deadline_remaining_seconds": (
                round(budget.deadline_at - time.monotonic(), 3) if budget else None
            ),
            "last_error": str(last_error) if last_error else None,
            "recent_trace_spans": _trace_spans[-20:],  # останні спани сесії для контексту
        }
        safe_ts = timestamp.replace(":", "-")
        filename = f"{fn_name}_{safe_ts}.json"
        path = os.path.join(_INCIDENTS_DIR, filename)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(report, f, ensure_ascii=False, indent=2)
        return path
    except Exception:
        return None  # postmortem не має сам ламати виконання програми


def _raise_exhausted(fn_name, budget, last_error):
    _write_incident_report(fn_name, budget, last_error)
    raise ResilienceExhausted(fn_name) from last_error


def list_recent_incidents(limit=10):
    """Для CLI/демонстрації - останні збережені post-mortem звіти."""
    if not os.path.exists(_INCIDENTS_DIR):
        return []
    files = sorted(os.listdir(_INCIDENTS_DIR), reverse=True)[:limit]
    return [os.path.join(_INCIDENTS_DIR, f) for f in files]


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


# ---------------------------------------------------------------------------
# Durable Execution - журнал на диску переживає навіть перезапуск процесу.
# Кожен resilient-виклик всередині активного durable workflow автоматично
# чекпоінтиться: якщо він вже успішно виконався в попередньому "запуску"
# (навіть якщо процес впав ПІСЛЯ цього кроку), при повторному виклику з тими
# самими аргументами крок береться з журналу без жодного реального I/O.
# ---------------------------------------------------------------------------

_JOURNAL_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".oberih_journal.json")


def _load_journal():
    if not os.path.exists(_JOURNAL_PATH):
        return {}
    try:
        with open(_JOURNAL_PATH, "r", encoding="utf-8") as f:
            return json.load(f)
    except (json.JSONDecodeError, OSError):
        return {}


def _save_journal(journal):
    with open(_JOURNAL_PATH, "w", encoding="utf-8") as f:
        json.dump(journal, f, ensure_ascii=False, indent=2)


def reset_durable_journal():
    """Для тестів/демо - очистити журнал (в реальному житті цього не роблять)."""
    if os.path.exists(_JOURNAL_PATH):
        os.remove(_JOURNAL_PATH)


class WorkflowContext:
    def __init__(self, workflow_id):
        self.id = workflow_id
        self.counter = 0


_current_workflow = contextvars.ContextVar("oberih_workflow", default=None)


class BudgetExceededError(NetworkFailure):
    def __init__(self, resource, fn_name):
        super().__init__(
            f"Бюджет ресурсу '{resource}' вичерпано під час '{fn_name}' - "
            f"подальші виклики в цьому ланцюжку заблоковано"
        )


def charge_budget(resource, amount, caller="llm.call"):
    """Списує amount одиниць ресурсу з АКТИВНОГО бюджету (Shared Budget).
    Якщо активного обмеження на цей ресурс немає - списання дозволене без
    перевірки (немає ліміту в цьому контексті). Якщо списання виводить
    залишок у мінус - кидає BudgetExceededError, блокуючи подальші витрати
    в межах цього ж ланцюжка викликів."""
    budget = _current_budget.get()
    if budget is None or resource not in budget.resources:
        return
    budget.resources[resource] -= amount
    if budget.resources[resource] < 0:
        raise BudgetExceededError(resource, caller)


class MockLLM:
    """Симулює виклик LLM з умовною вартістю в токенах і грошах -
    для демонстрації узагальненого бюджету без реального API-ключа."""
    def __init__(self):
        self.call_log = []

    def call(self, prompt):
        self.call_log.append(prompt)
        tokens = len(prompt) * 5  # умовна модель для демонстрації
        cost = round(tokens * 0.00002, 6)
        charge_budget("tokens", tokens)
        charge_budget("cost", cost)
        return f"[симульована відповідь LLM на: '{prompt[:40]}']"


llm_service = MockLLM()


# ---------------------------------------------------------------------------
# Вбудована обсервабельність - trace-спани створюються АВТОМАТИЧНО для будь-
# якого resilient-виклику, позначеного "traced", і для всіх вкладених у нього
# викликів (навіть без власного "traced") - на відміну від OpenTelemetry, де
# кожен спан треба створювати вручну в коді.
# ---------------------------------------------------------------------------

_trace_spans = []
_current_span = contextvars.ContextVar("oberih_span", default=None)


def get_trace_spans():
    return list(_trace_spans)


def clear_trace_spans():
    _trace_spans.clear()


def print_trace_tree():
    """Друкує дерево спанів з відступами за parent_id - для демонстрації."""
    by_parent = {}
    for span in _trace_spans:
        by_parent.setdefault(span["parent_id"], []).append(span)

    def _print(parent_id, depth):
        for span in by_parent.get(parent_id, []):
            print(
                f"{'  ' * depth}└─ {span['fn_name']} "
                f"[{span['outcome']}] {span['duration_ms']}ms"
            )
            _print(span["id"], depth + 1)

    _print(None, 0)


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


class SaturationRejectedError(NetworkFailure):
    def __init__(self, fn_name, current_limit):
        super().__init__(
            f"'{fn_name}' - адаптивний ліміт (зараз {current_limit:.1f}) заповнений, "
            f"система свідомо знижує навантаження через зростання затримки"
        )


# Стан адаптивного бекпрешеру persists між викликами (як і circuit breaker) -
# алгоритм AIMD (Additive-Increase/Multiplicative-Decrease), той самий дух,
# що й TCP Vegas / Netflix concurrency-limits: якщо затримка залишається
# близькою до найкращої коли-небудь спостереженої - ліміт повільно росте;
# якщо затримка різко зростає (ознака черги/перевантаження) чи стається
# збій - ліміт різко падає.
_adaptive_state = {}
_adaptive_lock = threading.Lock()


def _get_adaptive_state(fn_name, min_limit):
    with _adaptive_lock:
        if fn_name not in _adaptive_state:
            _adaptive_state[fn_name] = {
                "limit": float(min_limit),
                "in_flight": 0,
                "min_latency": None,
                "lock": threading.Lock(),
            }
        return _adaptive_state[fn_name]


def _adaptive_try_acquire(state):
    with state["lock"]:
        if state["in_flight"] >= state["limit"]:
            return False
        state["in_flight"] += 1
        return True


def _adaptive_release(state, min_limit, max_limit, latency, failed):
    with state["lock"]:
        state["in_flight"] -= 1
        if state["min_latency"] is None or latency < state["min_latency"]:
            state["min_latency"] = latency
        baseline = state["min_latency"] or latency
        queueing_detected = latency > baseline * 2.0
        if failed or queueing_detected:
            state["limit"] = max(min_limit, state["limit"] * 0.7)   # мультиплікативне зниження
        else:
            state["limit"] = min(max_limit, state["limit"] + 1)      # адитивне зростання


def adaptive_backpressure_snapshot():
    """Для дебагу/демонстрації - поточний стан усіх адаптивних лімітів."""
    with _adaptive_lock:
        return {
            k: {"limit": v["limit"], "in_flight": v["in_flight"], "min_latency": v["min_latency"]}
            for k, v in _adaptive_state.items()
        }


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
    def __init__(self, deadline_seconds, retry_budget, owner_fn, resources=None):
        self.deadline_at = time.monotonic() + deadline_seconds
        self.retries_left = retry_budget
        self.owner_fn = owner_fn
        self.attempts_log = []  # для демонстрації/дебагу
        self.resources = dict(resources or {})  # напр. {"tokens": 50000, "cost": 5.0}

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

# HTTP_MODE перемикає http.get між мок-мережею (для тестів, де ми навмисно
# симулюємо збої) і СПРАВЖНІМИ HTTP-запитами (для реального запуску .obh
# програм через CLI). Тести не чіпають цей прапорець - лишаються на "mock".
HTTP_MODE = "mock"

REPLAY_MODE = False  # у режимі replay реальне I/O заборонено - тільки дані з журналу


class RealHTTPClient:
    """Справжній HTTP-клієнт на стандартній бібліотеці urllib - без зайвих
    залежностей. JSON-відповіді автоматично розпарсюються в поля, щоб
    .obh-код міг одразу звертатись до response.fieldName."""

    def get(self, url, timeout=10):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Oberih/0.3"})
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                raw_body = resp.read().decode("utf-8", errors="replace")
                status = resp.status
        except urllib.error.HTTPError as e:
            raise NetworkFailure(f"HTTP {e.code} для {url}") from e
        except urllib.error.URLError as e:
            raise NetworkFailure(f"Мережева помилка для {url}: {e.reason}") from e
        except TimeoutError as e:
            raise NetworkFailure(f"Таймаут з'єднання для {url}") from e

        try:
            parsed = json.loads(raw_body)
        except json.JSONDecodeError:
            return {"status": status, "body": raw_body, "url": url}

        if isinstance(parsed, dict):
            parsed["_status"] = status
            parsed["_url"] = url
            return parsed
        return {"status": status, "value": parsed, "url": url}


real_http = RealHTTPClient()


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

    # --- Durable Execution: встановлюємо або успадковуємо workflow-контекст ---
    parent_workflow = _current_workflow.get()
    workflow_ctx = parent_workflow
    if "durable" in modifiers and parent_workflow is None:
        workflow_id = f"{fn_name}:{tuple(repr(a) for a in call_args)}"
        workflow_ctx = WorkflowContext(workflow_id)

    step_key = None
    journal = None
    if workflow_ctx is not None:
        workflow_ctx.counter += 1
        step_key = f"{workflow_ctx.id}::step{workflow_ctx.counter}::{fn_name}"
        journal = _load_journal()
        if step_key in journal:
            return journal[step_key]["value"]  # replay - жодного реального I/O
        if REPLAY_MODE:
            raise RuntimeError(
                f"REPLAY: крок '{step_key}' відсутній у журналі - "
                f"неможливо продовжити без реального виконання. "
                f"Ця гілка ніколи не виконувалась у записаному запуску."
            )

    workflow_token = _current_workflow.set(workflow_ctx)

    parent_span = _current_span.get()
    tracing_active = "traced" in modifiers or parent_span is not None
    span_id = f"span-{uuid.uuid4().hex[:8]}" if tracing_active else None
    span_start = time.monotonic()
    span_token = _current_span.set(span_id if tracing_active else parent_span)

    if "deadline" in modifiers or "retryBudget" in modifiers or "budget" in modifiers:
        budget = Budget(
            deadline_seconds=modifiers.get("deadline", 9999),
            retry_budget=modifiers.get("retryBudget", 9999),
            owner_fn=fn_name,
            resources=modifiers.get("budget"),
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
        saturating_cfg = modifiers.get("saturating")
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

            adaptive_state = None
            adaptive_acquired = False
            if saturating_cfg:
                adaptive_state = _get_adaptive_state(fn_name, saturating_cfg["min"])
                if not _adaptive_try_acquire(adaptive_state):
                    last_error = SaturationRejectedError(fn_name, adaptive_state["limit"])
                    break  # система сама вирішила знизити навантаження - не пробуємо
                adaptive_acquired = True

            sem = None
            if bulkhead_cfg:
                sem = _get_bulkhead_semaphore(fn_name, bulkhead_cfg["maxConcurrent"])
                if not sem.acquire(blocking=False):
                    last_error = BulkheadRejectedError(fn_name)
                    break  # усі "місця" зайняті - відхиляємо, не чекаючи в черзі

            if has_explicit_retries:
                budget.consume_attempt(fn_name)
            attempt += 1
            attempt_started_at = time.monotonic()
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
                if step_key is not None:
                    journal[step_key] = {"value": result}
                    _save_journal(journal)
                return result
            except NetworkFailure as e:
                last_error = e
                if breaker_cfg:
                    _breaker_record_failure(fn_name, breaker_cfg)
                continue
            finally:
                if sem is not None:
                    sem.release()
                if adaptive_acquired:
                    latency = time.monotonic() - attempt_started_at
                    failed = sys.exc_info()[0] is not None
                    _adaptive_release(
                        adaptive_state, saturating_cfg["min"], saturating_cfg["max"],
                        latency, failed=failed,
                    )

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
                _raise_exhausted(fn_name, budget, last_error)

        if fallback_defined:
            # fallback був, але теж провалився, emergency відсутній
            _raise_exhausted(fn_name, budget, last_error)

        if last_error is not None:
            raise last_error
        _raise_exhausted(fn_name, budget, last_error)
    finally:
        _current_budget.reset(token)
        _current_workflow.reset(workflow_token)
        if tracing_active:
            exc_type = sys.exc_info()[0]
            if exc_type is None:
                outcome = "success"
            elif exc_type is ResilienceExhausted:
                outcome = "exhausted"
            else:
                outcome = "error"
            _trace_spans.append({
                "id": span_id,
                "parent_id": parent_span,
                "fn_name": fn_name,
                "duration_ms": round((time.monotonic() - span_start) * 1000, 2),
                "outcome": outcome,
            })
        _current_span.reset(span_token)
