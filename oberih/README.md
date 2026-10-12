# Oberih (Оберіг)

> Мова програмування, у якій відмовостійкість — частина синтаксису, а не бібліотека зверху.

Назва **Oberih** походить від українського слова "оберіг" — талісман, що захищає від
біди. Так само мова захищає твій код від збоїв: мережевих помилок, таймаутів,
каскадних падінь сервісів — без ручного написання retry/timeout/circuit-breaker
логіки на кожен виклик.

**Статус: робочий прототип.** Це не тільки специфікація - мова реально парсить,
типізує (градуально) і виконує `.obh`-файли. Усі приклади в `examples/` і всі
тести в `tests/` реально проходять (`python3 tests/run_all.py`).

## Швидкий старт

```bash
pip install -r requirements.txt
python3 oberih.py run examples/04_control_flow.obh
```

## Проблема, яку вирішує Oberih

Сучасні розподілені системи (мікросервіси, зовнішні API, хмарна інфраструктура)
постійно стикаються з частковими збоями. Стандартний підхід — обгортати кожен
виклик вручну в try/catch чи бібліотеки типу Polly/Hystrix/resilience4j, і
сподіватись, що ніхто не забув про edge-case.

Найгірший з цих edge-case — **retry storm**: коли вкладені виклики з власними
retry-політиками множаться, довершуючи падіння вже перевантаженого сервісу.

**Oberih вирішує це на рівні мови**, а не бібліотеки, і компілятор **фізично
не дозволяє** скомпілювати код, вразливий до retry storm.

## Ключова ідея: Shared Budget

Весь ланцюжок викликів має **єдиний бюджет** часу (`deadline`) і спроб
(`retryBudget`), який передається вниз автоматично - так само, як
`context.Context` передає дедлайн у Go.

```oberih
resilient fn getOrderSummary(orderId: String) -> Result<OrderSummary, ResilienceExhausted>
    deadline(2s)
    retryBudget(6)
    fallback(cachedSummary(orderId))
{
    let order = fetchOrder(orderId)?
    let user = fetchUser(order.userId)?
    let payment = fetchPaymentStatus(order.id)?
    return Ok(OrderSummary(order, user, payment))
}
```

Вкладені виклики можуть оголошувати власний `retries(n)`, але це стеля,
обмежена залишком спільного бюджету, а не додаткові спроби. **Компілятор
вимагає** явний `deadline`/`retryBudget` на кожній кореневій resilient fn -
без цього код просто не скомпілюється (перевірено в `examples/05_BAD_missing_deadline.obh`).

## Усі модифікатори стійкості (реалізовано і перевірено тестами)

| Модифікатор | Що робить | Тест |
|---|---|---|
| `retries(n)` | Стеля спроб у межах спільного бюджету | `tests/demo_shared_budget.py` |
| `deadline(duration)` | Спільний часовий бюджет ланцюжка | `tests/run_example_01.py` |
| `retryBudget(n)` | Спільний бюджет спроб ланцюжка | `tests/run_example_01.py` |
| `fallback(expr)` | Значення при вичерпанні бюджету | `tests/run_example_01.py` |
| `timeout(duration)` | Примусово перериває довгу спробу | `tests/run_example_timeout.py` |
| `circuitBreaker(failThreshold:, cooldown:)` | Блокує виклики до сервісу, що падає | `tests/run_example_02.py` |
| `idempotent(key:)` | Не виконує повторно з тим самим ключем | `tests/run_example_02.py` |
| `cache(ttl:)` | Уникає повторного мережевого виклику | вбудовано в `examples/03` |
| `emergencyFallback(expr)` | Гарантований рубіж без I/O (компілятор це перевіряє) | `tests/run_example_emergency.py` |
| `rateLimit(n, per:)` | Обмежує частоту викликів у часі | `tests/run_example_ratelimit.py` |
| `bulkhead(maxConcurrent:)` | Обмежує одночасні виклики (реальні потоки) | `tests/run_example_bulkhead.py` |
| `hedging(after:)` | Паралельна дублююча спроба при затримці | `tests/run_example_hedging.py` |

## Обробка помилок: провал fallback

Якщо основний `fallback` теж провалюється, а `emergencyFallback` не заданий -
кидається `ResilienceExhausted`. Якщо `emergencyFallback` заданий - він
спрацьовує як останній гарантований рубіж. Компілятор статично забороняє
мережеві виклики всередині `emergencyFallback` (перевірено в typechecker).

## Мова загального призначення, не тільки DSL

Крім модифікаторів стійкості, Oberih має звичайні конструкції: `if/else`,
`while`, змінні, арифметику, конкатенацію рядків. Функції без ключового слова
`resilient` виконуються напряму, без вимоги deadline/retryBudget:

```oberih
fn classify(amount: Number) -> String {
    if (amount > 1000) {
        return "large"
    } else {
        return "small"
    }
}
```

Дивись `examples/04_control_flow.obh`.

## Типова система (градуальна)

- **Статична типізація для примітивів**: `Number`, `String`, `Boolean`
  реально перевіряються компілятором до виконання (`typechecker.py`).
- Будь-який інший тип (`Result<T,E>`, `Order`, `Money`, ...) трактується як
  `Dynamic` - ми свідомо не блокуємо компіляцію там, де не маємо структурних
  означень типів. Повна структурна типізація з дженериками - наступна ітерація.
- `Result<T,E>` наразі моделюється через Python-виключення під капотом, а не
  окремий тип даних; оператор `?` синтаксично присутній, семантично прозорий.

Приклад реальної помилки типів, яку ловить компілятор, - `examples/06_BAD_type_errors.obh`.

## Модель виконання

Oberih - незалежна мова, без транспіляції в інші мови.

- **Поточний стан**: tree-walking interpreter (`evaluator.py`) - виконує AST
  напряму. Working CLI: `oberih.py run file.obh`.
- **Наступна фаза**: компіляція у власний байткод + стекова віртуальна
  машина - коли мова матиме користувачів і зрозумілі патерни навантаження.

## Структура репозиторію

```
oberih/
├── grammar.lark          # формальна граматика мови (Lark, LALR)
├── evaluator.py          # AST-виконавець (парсинг -> виконання)
├── interpreter.py        # рантайм: Shared Budget, circuitBreaker, cache, ...
├── typechecker.py        # статичні перевірки (типи + deadline на корені + emergencyFallback)
├── oberih.py             # CLI: python3 oberih.py run file.obh
├── requirements.txt
├── examples/             # робочі .obh файли, включно з навмисно "поганими"
├── tests/                # усі тести, запускаються через tests/run_all.py
└── docs/
    └── ARCHITECTURE.md   # детальний опис внутрішньої архітектури
```

## Розширення файлів

`.obh`

## Чесні обмеження поточної версії

- Немає структурних означень типів (struct/class) - складні типи трактуються
  як `Dynamic` і не перевіряються повністю.
- `Result<T,E>`/`Option<T>` не окремий тип даних, а шар поверх виключень.
- `bulkhead` - варіант "reject if full", без черги очікування.
- `hedging` не справді скасовує "програшну" паралельну спробу (вона
  доviконується у фоні) - прийнятно для мок-мережі, для реального I/O
  потрібне справжнє скасування (cancellation).
- Compile-time перевірка "root resilient fn" не є flow-sensitive: якщо
  функція викликається і як коренева, і як вкладена в різних місцях коду,
  статичний аналіз консервативно вважає її вкладеною (див. коментар у
  `typechecker.py`).
