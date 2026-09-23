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

// WeakRef для циклічних структур
let weak = weakRef(myList)
let result = upgrade(weak)

// Вбудований тест-ранер
fn testAdd() -> Bool { return add(1, 2) == 3 }
```

## Стандартна бібліотека

| Категорія | Функції |
|-----------|---------|
| IO | `print`, `println`, `readLine`, `readFile`, `writeFile`, `appendFile` |
| HTTP | `httpGet`, `httpPost`, `httpPut`, `httpDelete` |
| Рядки | `strLen`, `strTrim`, `strUpper`, `strLower`, `strSplit`, `strJoin`, `strReplace`, `strSlice`, `strContains`, `strStartsWith`, `strEndsWith` |
| Числа | `floor`, `ceil`, `round`, `abs`, `sqrt`, `pow`, `min`, `max` |
| Списки | `len`, `push`, `pop`, `first`, `last`, `reverse`, `contains`, `range` |
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
| HTTP клієнт | ✅ |
| Форматер (oberih fmt) | ✅ |
| Тест-ранер (oberih test) | ✅ |
| Людські повідомлення про помилки | ✅ |
| Стандартна бібліотека | ✅ |
