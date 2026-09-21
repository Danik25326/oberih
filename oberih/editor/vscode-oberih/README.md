# Oberih Language Support для VS Code

Підсвітка синтаксису + **справжній Language Server (LSP)** для файлів `.obh`:
- Помилки компілятора (той самий `deadline`/`retryBudget`/типи/emergencyFallback)
  показуються прямо в редакторі, з підкресленням, без запуску CLI вручну
- Автодоповнення ключових слів, модифікаторів стійкості, вбудованих об'єктів і типів

## Встановлення (у VS Code Desktop або GitHub Codespaces)

1. Встанови залежність LSP-клієнта:

```bash
cd editor/vscode-oberih
npm install vscode-languageclient
```

2. Скопіюй папку `editor/vscode-oberih` в `~/.vscode/extensions/` (локально)
   або `~/.vscode-server/extensions/` (у Codespaces):

```bash
mkdir -p ~/.vscode-server/extensions/oberih-language-0.2.0
cp -r editor/vscode-oberih/* ~/.vscode-server/extensions/oberih-language-0.2.0/
```

3. Перезавантаж вікно VS Code: `Ctrl+Shift+P` → "Developer: Reload Window".

4. Відкрий будь-який `.obh` файл — підсвітка і LSP увімкнуться автоматично.
   Розширення саме запускає `python3 lsp_server.py` у фоні.

## Що працює через LSP

- **Діагностика в реальному часі**: відкрий файл без `deadline` на кореневій
  resilient-функції - побачиш червоне підкреслення й повідомлення прямо в
  редакторі, без запуску `oberih.py run`.
- **Автодоповнення**: почни вводити `dead` → з'явиться `deadline` у списку.

Перевірено окремим тестом (`tests/run_example_lsp.py`), який спілкується
з сервером через справжній JSON-RPC/stdio протокол - той самий, яким
користується VS Code.

## Що підсвічується (TextMate-граматика)

- **Ключові слова**: `fn`, `resilient`, `struct`, `if`, `else`, `while`, `for`, `let`, `return`
- **Модифікатори стійкості**: `deadline`, `retryBudget`, `retries`, `fallback`,
  `circuitBreaker`, `idempotent`, `cache`, `timeout`, `rateLimit`, `bulkhead`,
  `hedging`, `budget`, `durable`, `traced`, `emergencyFallback`
- **Вбудовані об'єкти**: `http`, `paymentGateway`, `weatherApi`, `llm`, `agent`
- **Типи**: `Number`, `String`, `Boolean`, `Result`, `Option`, `List`, `Map`
- **Рядки, числа, тривалості** (`2s`, `500ms`, `1m`)
- **Коментарі** (`// ...`)

## Чесне обмеження

Немає "go to definition", "hover", перейменування змінних. Помилки
typechecker (на відміну від синтаксичних) прив'язані до першого рядка
файлу, бо наразі не несуть власних координат у AST - точна локалізація
кожної типової помилки в редакторі - наступний крок.

