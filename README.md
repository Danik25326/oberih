# Oberih

Мова програмування з вбудованою стійкістю до збоїв.
Власний рантайм на Rust — один бінарний файл, нуль залежностей.

```oberih
resilient fn getOrder(id: String) -> String
    deadline(2s)
    retryBudget(3)
    fallback(cachedOrder)
{
    let result = fetchOrder(id)?
    return result
}
```

## Збірка

```bash
cargo build --release
./target/release/oberih run examples/01_microservices_chain.obh
```

## Команди

```bash
oberih run <file.obh>       — виконати програму
oberih check <file.obh>     — перевірити синтаксис
oberih tokens <file.obh>    — показати токени
oberih ast <file.obh>       — показати AST
oberih bytecode <file.obh>  — показати bytecode
```

## Архітектура

```
src/
├── lexer/          — лексер
├── parser/
│   ├── ast.rs      — AST
│   └── mod.rs      — recursive descent парсер
├── compiler/
│   ├── bytecode.rs — інструкції VM
│   └── mod.rs      — AST → bytecode
└── vm/
    └── mod.rs      — stack-based VM
```

## Статус

| Компонент | Статус |
|-----------|--------|
| Лексер | ✅ |
| Парсер | ✅ |
| Компілятор AST → bytecode | ✅ |
| VM: арифметика, if/while/for | ✅ |
| VM: Result\<T,E\> і ? | ✅ |
| VM: spawn / join / cancel | ✅ |
| VM: resilience (deadline/retry/circuit breaker) | ✅ |
| VM: struct з іменованими полями | 🔄 |
| Typechecker | ❌ |
| explain (Shared Budget tree) | ❌ |

---

## Roadmap до 9/10

| # | Що | Чому важливо |
|---|-----|-------------|
| 1 | **Справжня generic інференція** | Зараз `T` просто сумісний з усім — typechecker не ловить помилки в generic функціях |
| 2 | **Garbage collector** | Зараз пам'ять не збирається — довгі програми з циклами і списками течуть |
| 3 | **Повний тест 19 прикладів** | Деякі edge cases в компіляторі можуть давати збої на реальних `.obh` файлах |

