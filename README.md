# Oberih

Мова програмування з вбудованою стійкістю до збоїв.  
Власний рантайм на Rust — один бінарний файл, нуль залежностей.

```oberih
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retries(3)
    fallback(cachedOrder)
{
    let result = fetchOrder(id)?
    return result
}
```

## Збірка

```bash
cargo build --release
.\target\release\oberih.exe run examples\cli_tool.obh localhost:8080
```

## Команди

```bash
oberih run      <файл>          — виконати програму
oberih repl                     — інтерактивний режим
oberih check    <файл>          — перевірити синтаксис і типи
oberih fmt      <файл>          — форматувати код
oberih test     <файл>          — запустити тести (fn test*)
oberih explain  <файл> [fn]     — показати дерево Shared Budget
oberih tokens   <файл>          — показати токени
oberih ast      <файл>          — показати AST
oberih bytecode <файл>          — показати bytecode
oberih version                  — версія
oberih gc-stats                 — статистика garbage collector
```

## Архітектура

```
src/
├── main.rs         — CLI точка входу
├── lexer/          — власний лексер, UTF-8
├── parser/
│   ├── ast.rs      — повне AST
│   └── mod.rs      — recursive descent парсер
├── compiler/
│   ├── bytecode.rs — інструкції VM + ResilienceMeta
│   ├── struct_table.rs — таблиця struct типів
│   └── mod.rs      — AST → bytecode компілятор
├── vm/
│   └── mod.rs      — stack-based VM
├── gc.rs           — reference counting GC + WeakRef
├── typechecker.rs  — статична перевірка типів
├── explain.rs      — Shared Budget tree аналіз
├── stdlib.rs       — стандартна бібліотека
├── diagnostics.rs  — людські повідомлення про помилки
├── formatter.rs    — oberih fmt
└── test_runner.rs  — oberih test
```

## Що вміє мова

```oberih
// Resilience як синтаксис
resilient fn fetch(url: String) -> String
    deadline(2s)
    retries(3)
    fallback(cached)
{ ... }

// Result<T,E> і ? оператор
let data = fetch("url")?

// Struct з reference semantics
struct Point { x: Number, y: Number }
fn Point.distance(self: Point) -> Number { ... }

// Generics
struct Box<T> { value: T }

// Pattern matching
let msg = match result {
    Ok(v)  => "отримали: " + v,
    Err(e) => "помилка: " + e,
    _      => "невідомо"
}

// Паралельне виконання
let h1 = spawn fetchA("url1")
let h2 = spawn fetchB("url2")
let r1 = h1.join()

// HTTP клієнт
let resp = httpGet("http://api.example.com/data")?
println(resp.body)

// Логічні оператори
let a = true && false   // false
let b = true || false   // true
let c = !true           // false

// Модульна система
import "lib/math.obh"
import "lib/strings.obh"

let result = add(10, 5)        // з math.obh
let s = capitalize("oberih")   // з strings.obh
let result = upgrade(weak)

// Вбудований тест-ранер
fn testAdd() -> Bool { return add(1, 2) == 3 }
```

## Map (словник)

```
let user = {"name": "Оксана", "age": 30}     // літерал; {} — порожній
user["city"] = "Київ"                         // індексація
user.age = user.age + 1                       // user.key == user["key"]
let n = user["нема"]                          // відсутній ключ -> nil
mapGet(user, "zip", "-")                      // значення за замовчуванням
for (k in user) { println(k, user[k]) }       // for перебирає ключі
```

- Ключі: `String`, `Number` або `Bool` (інші — помилка). Порядок вставки зберігається.
- Map — **посилальний тип** (як struct і list): `let b = a` не копіює, зміни бачать усі.
- Рівність `==` не залежить від порядку ключів.
- `mapMerge(a, b)` повертає **новий** Map; `mapSet`/`mapDelete` змінюють на місці — обидва амортизовано O(1).
- Типи: `Map<String, Number>` у параметрах/поверненні; typechecker перевіряє ключі й значення.
- `jsonParse` повертає `Map` для об'єктів, тож `resp.info.summary` працює як раніше.

## Лямбди й замикання

```
let double = fn(x) => x * 2                 // вираз-тіло
let clamp = fn(x: Number) -> Number {         // блок-тіло, з типами
    if (x < 0) { return 0 }
    return x
}
map(xs, fn(x) => x * 2)                       // функції вищого порядку приймають лямбди
```

- Захоплення — **за значенням** у момент створення лямбди; List/Map/struct — посилальні типи, тому зміни їхнього вмісту видно й через замикання.
- Типи функцій: `Fn` (будь-яка) або `Fn(Number, String) -> Bool` (конкретна сигнатура) — у параметрах і полях.
- Лямбди зберігаються між рядками REPL так само, як звичайні `let`.

## Регулярні вирази

Власний рушій (Pike VM) — час пошуку лінійний за розміром тексту й шаблону, тож
катастрофічний backtracking (ReDoS) неможливий, на відміну від naive backtracking-рушіїв.

```
let re = regex("(\\d+)-(\\d+)")?           // компіляція; Err при некоректному шаблоні
re.test(s)                                  // Bool
let m = re.find(s)                          // nil або {text, start, end, groups}
re.findAll(s)                               // List таких Map
re.replace(s, "$2-$1")   re.replaceAll(...)  // $1, $2 — посилання на групи; $$ — літеральний $
re.split(s)
```

Скорочення для одноразового використання: `reTest`, `reFind`, `reFindAll`, `reReplace`, `reReplaceAll`, `reSplit` (кожен викликом компілює шаблон заново — для циклів компілюйте через `regex()` один раз).

Підтримка: `. [] [^] \\d \\w \\s \\D \\W \\S \\b \\B ^ $ ( ) (?: ) | * + ? {n} {n,m}` (жадібні й ліниві `*? +? ??`), `(?i)` на початку шаблону. Без зворотних посилань, lookahead/lookbehind і іменованих груп.

## Пакетний менеджер

```
oberih init my-app                              # oberih.toml + src/main.obh
cd my-app
oberih add mathutils --path ../mathutils        # локальна залежність
oberih add web --git https://github.com/u/web.git --branch main
oberih run                                      # без аргументу — бере entry з oberih.toml
```

`oberih.toml`:
```toml
[package]
name    = "my-app"
version = "0.1.0"
entry   = "src/main.obh"

[dependencies]
mathutils = { path = "../mathutils" }
web       = { git = "https://github.com/u/web.git", branch = "main" }
```

- Це навмисно НЕ повний TOML — рядкова підмножина, якої досить для цієї схеми (секції, `ключ = "рядок"`, інлайн-таблиці `{ }`, коментарі `#`).
- `path`-залежність береться як є; `git` клонується (мілко, `--depth 1`) у `.oberih/deps/<ім'я>`. `tag`/`rev` фіксують версію назавжди (кеш більше не оновлюється); без них кожен запуск оновлює до останнього коміту гілки.
- У коді залежність використовується за іменем: `import "mathutils/utils.obh"` розв'язується відносно кореня залежності `mathutils`, а не поточного файлу.
- `run`/`check`/`test`/`explain` без аргументу файлу шукають `oberih.toml` вгору по директоріях (як `Cargo.toml`) і використовують `entry`.
- Реєстру пакетів немає — лише `path` і `git`. Версія на кшталт `mathutils = "1.0"` — явна помилка з поясненням, а не мовчазне ігнорування.

## REPL

```
>>> let x = 5
= 5
>>> if (x > 0) {
...   print("додатне")
... }
додатне
>>> :history
    1  let x = 5
    2  if (x > 0) {  ⏎ print("додатне")  ⏎ }
>>> !1
= 5
```

- **Багаторядковий ввід:** незакрита `(`, `[` чи `{` продовжує читання з підказкою `... `, доки дужки не збалансуються. Рахується на рівні токенів лексера, тож дужка всередині рядка чи коментаря не рахується.
- **Історія:** `:history` показує команди поточного й попередніх сеансів (зберігається у `~/.oberih_history`); `!N` повторює запис №N, `!!` — останній.
- Немає навігації стрілками вгору/вниз по історії під час набору — це вимагало б "сирого" режиму термінала і нової залежності (rustyline/crossterm), а REPL свідомо тримається нуля залежностей поза TLS.

## Глибока рекурсія: контрольована помилка замість краху процесу

Раніше `fn sum(n) { return n + sum(n-1) }` обвалював ВЕСЬ процес (`stack
overflow`, SIGABRT) вже на ~2500 рівнях рекурсії — типового стека головного
потоку (8 МБ) на це не вистачало. Тепер:

- Код насправді виконується на окремому потоці з 512 МБ стека (`run`, `test`, REPL).
- Глибина викликів рахується і обмежена `MAX_CALL_DEPTH` (150 000): вище —
  звичайна `RuntimeError`, яку можна зловити через `Result`, а не крах.
- Процес після такої помилки працює далі як зазвичай (перевірено: наступний
  виклик у тому ж REPL чи тестовому прогоні виконується нормально).

**Чесне обмеження:** це НЕ переписування VM на явний стек кадрів. Глибина й
далі обмежена (щедрим) стеком ОС, а не лише пам'яттю — просто межа тепер
набагато вища (150 000 замість ~2500) і падіння контрольоване. Справжня
необмежена рекурсія (і асинхронність, якій вона потрібна як фундамент)
вимагає переписати виконавець на явний стек викликів — це вже окрема, значно
більша робота, не зроблена тут.

## Інструменти розробника

Чесно: повноцінний LSP-сервер (підсвітка/автодоповнення в редакторі) чи
дебагер — це тижні окремої роботи, їх тут немає. Що є:

```
oberih docs                  # довідка по stdlib у stdout (Markdown)
oberih docs reference.md     # те саме, у файл
```

Генерується зі структурованої таблиці (`src/docs_data.rs`), а не з коментарів
у коді — 126 функцій, кожна з сигнатурою, описом і прикладом. Таблицю
ведуть вручну (синхронізація зі списком builtin у `stdlib.rs` не
автоматична), але тест гарантує, що кожне задокументоване ім'я справді існує.

## Бітові оператори

```
6 & 3        // 2   AND
6 | 1        // 7   OR
6 ^ 5        // 3   XOR
~5           // -6  NOT (двійкове доповнення)
1 << 8       // 256 зсув вліво
256 >> 4     // 16  зсув вправо
```

- Операнди — цілі `Number` у межах ±2^53 (межа точного представлення f64); дробові чи виходять за межу — помилка виконання, а не мовчазне усічення.
- **Пріоритет — як у Python, не як у C**: `| ^ &` тісніші за порівняння, але слабші за зсуви. `1 | 2 == 3` означає `(1 | 2) == 3`. Зсуви `<<`/`>>` слабші за `+ -`, тож `2 << 1 + 1` це `2 << (1 + 1)`.
- `>>` навмисно не окремий токен лексера — інакше зламалось би закриття вкладених дженериків (`Map<String, List<Number>>` закінчується двома символами `>`). Парсер розпізнає дві сусідні `>` як зсув лише на рівні виразів, де дженериків немає.

## Збирач циклів посилань

Reference counting (у списках/Map/struct) не звільняє цикли:
```
let a = {"val": 1}
let b = {"val": 2}
a["next"] = b
b["prev"] = a
// a, b більше нізвідки не досяжні, але тримають одне одного —
// звичайний refcounting НІКОЛИ їх не звільнить.
```
- У **REPL** цикли розриваються автоматично після кожного рядка (корені — всі змінні сесії), і їх показує `:gc`.
- У коді — явно, вбудованою функцією: `gcCollectCycles(живе1, живе2, ...)` — усе передане лишається живим, решта зареєстрованого й недосяжного звідси звільняється. `gcStats()` повертає `{allocs, drops, live}`.
- Обмеження: через рекурсивну модель викликів VM неможливо безпечно автоматично зібрати цикли ПОСЕРЕД виконання довільної програми (на відміну від REPL, де між рядками немає нічого, крім змінних сесії) — `gcCollectCycles(...)` варто викликати в однопотоковій частині коду, без паралельних `spawn`, що звертаються до тих самих даних.

## Стандартна бібліотека

| Категорія | Функції |
|-----------|---------|
| IO | `print`, `println`, `readLine`, `readFile`, `writeFile`, `appendFile` |
| HTTP/HTTPS | `httpGet`, `httpPost`, `httpPut`, `httpDelete` (http:// і https://) |
| JSON | `jsonParse`, `jsonStringify`, `jsonPretty` (JSON-об'єкт ↔ `Map`) |
| Рядки | `strLen`, `strTrim`, `strUpper`, `strLower`, `strSplit`, `strJoin`, `strReplace`, `strSlice`, `strContains`, `strStartsWith`, `strEndsWith` |
| Числа | `floor`, `ceil`, `round`, `abs`, `sqrt`, `pow`, `min`, `max` |
| Списки | `len`, `push` (новий список), `append` (на місці), `pop`, `first`, `last`, `reverse`, `contains`, `range`, `slice`, `concat`, `indexOf`, `sort`, `sum` |
| Функції вищого порядку | `map`, `filter`, `reduce`, `each`, `any`, `all`, `find`, `sortBy` — приймають ім'я функції: `map(xs, double)` |
| Map | `len`, `keys`, `values`, `entries`, `mapHas`, `mapGet`, `mapSet`, `mapDelete`, `mapMerge` (і методи: `m.keys()`, `m.has(k)`, `m.get(k, def)`…) |
| Типи, числа | `typeOf`, `parseNumber` (безпечно → Result), `sin`, `cos`, `tan`, `log`, `exp`, `pi`, `sign`, `random`, `randInt`, `seedRandom` |
| Рядки (додатково) | `strIndexOf`, `strRepeat`, `strPadLeft`, `strPadRight` |
| Час | `now`, `isoTime`, `dateParts`, `timeFromParts` (усі — з необов'язковим зсувом у хвилинах: `isoTime(t, 180)`), `parseIso` |
| Файли (додатково) | `fileExists`, `readLines`, `listDir`, `deleteFile`, `mkdir` |
| Час | `now`, `sleep` |
| Процес | `args`, `env`, `exit` |
| GC | `weakRef`, `upgrade`, `isAlive`, `gcstats` |
| Відладка | `debug`, `assert`, `panic`, `toString`, `toNumber`, `toBool` |

## Статус

| Компонент | Статус |
|-----------|--------|
| Лексер | ✅ |
| Парсер | ✅ |
| Компілятор AST → bytecode | ✅ |
| VM: арифметика, if/while/for | ✅ |
| VM: Result\<T,E\> і ? | ✅ |
| VM: struct з іменованими полями | ✅ |
| VM: spawn / join | ✅ |
| VM: resilience (deadline/retry/circuit breaker/rate limit/bulkhead) | ✅ |
| Typechecker | ✅ |
| Generic інференція | ✅ |
| explain (Shared Budget tree) | ✅ |
| Garbage collector (Arc reference counting) | ✅ |
| WeakRef (циклічні посилання) | ✅ |
| HTTP клієнт (http://) | ✅ |
| HTTPS клієнт (https:// via rustls; додаткові CA через `SSL_CERT_FILE`) | ✅ |
| JSON парсер і серіалізатор | ✅ |
| Модульна система (import, `private` fn/struct перевіряється між файлами) | ✅ |
| enum (`Status.Active`, `==`, `match`) | ✅ |
| Map (`{"k": v}`, `m[k]`, `m.k`, `for`, методи, JSON) | ✅ |
| Лямбди й замикання (`fn(x) => x*2`, `fn(x) -> T {...}`) | ✅ |
| Типи функцій (`Fn`, `Fn(Number) -> Bool`) | ✅ |
| Регулярні вирази (Pike VM, лінійний час, без ReDoS) | ✅ |
| Пакетний менеджер (`oberih.toml`, `init`, `add`, залежності path/git) | ✅ |
| Анотація типу в `let` (`let m: Map<String, Number> = ...`) | ✅ |
| Бітові оператори (`& | ^ ~ << >>`, пріоритет як у Python) | ✅ |
| Збирач циклів посилань (`gcCollectCycles`, автоматично в REPL) | ✅ |
| Логічні оператори (&& \|\| !) | ✅ |
| REPL (зберігає стан, багаторядковий ввід, історія: `:vars`, `:fns`, `:history`, `!N`) | ✅ |
| Форматер (oberih fmt; зберігає коментарі, ідемпотентний) | ✅ |
| Тест-ранер (oberih test) | ✅ |
| Людські повідомлення про помилки | ✅ |
| Стандартна бібліотека | ✅ |

## HTTPS за корпоративним проксі

Клієнт довіряє вбудованому набору CA (`webpki-roots`). Якщо мережа підміняє
сертифікати власним CA, вкажіть його PEM-файл (або системний бандл):

```
SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt oberih run examples/07_real_network.obh
```

## Відомі обмеження

- **Часові пояси:** тільки фіксовані зсуви від UTC у хвилинах (`isoTime(t, 180)`), без бази iana/tzdata. Іменовані пояси (`Europe/Kyiv` з переходом на літній час) не підтримуються — це вимагало б окремої залежності з базою даних поясів; робити її "приблизно правильною" самотужки гірше, ніж чесно її не мати.
- **Регулярні вирази:** без зворотних посилань (`\\1`), lookahead/lookbehind і іменованих груп — рушій свідомо обмежений до конструкцій, що гарантують лінійний час.
- **Вивід контейнерів:** рядки в списках/Map беруться в лапки (`["a", 1]`), але немає керування форматом (ширина, кастомний Display для struct).
