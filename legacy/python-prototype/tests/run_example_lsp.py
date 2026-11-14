"""
Тестує lsp_server.py як РЕАЛЬНИЙ окремий процес, спілкуючись з ним через
той самий stdio JSON-RPC протокол, яким користується VS Code. Це
найпереконливіша перевірка з можливих без справжнього редактора: сервер
або відповідає правильним протоколом, або тест провалюється.
"""

import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
_ROOT = os.path.join(os.path.dirname(__file__), "..")

import subprocess
import json


def _frame(obj):
    body = json.dumps(obj)
    data = body.encode("utf-8")
    return f"Content-Length: {len(data)}\r\n\r\n".encode("utf-8") + data


def _read_one_message(proc):
    headers = {}
    while True:
        line = proc.stdout.readline()
        if not line:
            return None
        line = line.decode("utf-8").rstrip("\r\n")
        if line == "":
            break
        key, _, value = line.partition(":")
        headers[key.strip()] = value.strip()
    length = int(headers.get("Content-Length", 0))
    body = proc.stdout.read(length).decode("utf-8")
    return json.loads(body)


def main():
    server_path = os.path.join(_ROOT, "lsp_server.py")
    proc = subprocess.Popen(
        [sys.executable, server_path],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )

    def send(obj):
        proc.stdin.write(_frame(obj))
        proc.stdin.flush()

    print("=" * 70)
    print("КРОК 1: initialize handshake")
    print("=" * 70)
    send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})
    resp = _read_one_message(proc)
    print("Відповідь сервера:", resp)
    assert resp["id"] == 1
    assert "completionProvider" in resp["result"]["capabilities"]
    print(">>> initialize OK")

    send({"jsonrpc": "2.0", "method": "initialized", "params": {}})

    print()
    print("=" * 70)
    print("КРОК 2: didOpen з НАВМИСНЕ поганим кодом (без deadline)")
    print("Очікується: publishDiagnostics з реальною помилкою компілятора")
    print("=" * 70)
    bad_source = (
        'resilient fn getPrice(itemId: String) -> Result<Price, Error>\n'
        '    retries(3)\n'
        '{\n'
        '    return http.get("/prices/" + itemId)\n'
        '}\n'
    )
    send({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": "file:///test.obh",
                "text": bad_source,
            }
        },
    })
    notif = _read_one_message(proc)
    print("Отримана нотифікація:", notif["method"])
    diags = notif["params"]["diagnostics"]
    print(f"Кількість діагностик: {len(diags)}")
    print("Повідомлення:", diags[0]["message"])
    assert notif["method"] == "textDocument/publishDiagnostics"
    assert len(diags) == 1
    assert "deadline" in diags[0]["message"]
    print(">>> Реальна помилка компілятора отримана через LSP-протокол: ТАК")

    print()
    print("=" * 70)
    print("КРОК 3: didChange з ВИПРАВЛЕНИМ кодом")
    print("Очікується: publishDiagnostics з ПОРОЖНІМ списком (помилок нема)")
    print("=" * 70)
    good_source = (
        'resilient fn getPrice(itemId: String) -> Result<Price, Error>\n'
        '    deadline(5s)\n'
        '    retryBudget(3)\n'
        '{\n'
        '    return http.get("/prices/" + itemId)\n'
        '}\n'
    )
    send({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {"uri": "file:///test.obh"},
            "contentChanges": [{"text": good_source}],
        },
    })
    notif = _read_one_message(proc)
    diags = notif["params"]["diagnostics"]
    print(f"Кількість діагностик після виправлення: {len(diags)}")
    assert len(diags) == 0
    print(">>> Помилки зникли після виправлення коду: ТАК")

    print()
    print("=" * 70)
    print("КРОК 4: автодоповнення (completion)")
    print("=" * 70)
    send({"jsonrpc": "2.0", "id": 2, "method": "textDocument/completion", "params": {
        "textDocument": {"uri": "file:///test.obh"},
        "position": {"line": 0, "character": 0},
    }})
    resp = _read_one_message(proc)
    labels = [item["label"] for item in resp["result"]]
    print(f"Кількість елементів автодоповнення: {len(labels)}")
    print("Приклади:", [l for l in labels if l in ("resilient", "deadline", "durable", "agent", "List")])
    assert "resilient" in labels
    assert "deadline" in labels
    assert "durable" in labels
    assert "agent" in labels
    print(">>> Автодоповнення містить реальні ключові слова мови: ТАК")

    print()
    print("=" * 70)
    print("КРОК 5: коректне завершення")
    print("=" * 70)
    send({"jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": {}})
    resp = _read_one_message(proc)
    assert resp["id"] == 3
    send({"jsonrpc": "2.0", "method": "exit", "params": {}})
    proc.wait(timeout=5)
    print(f">>> Сервер коректно завершився, код виходу: {proc.returncode}")

    print()
    print("=" * 70)
    print("УСІ КРОКИ LSP-ТЕСТУ ПРОЙШЛИ УСПІШНО")
    print("=" * 70)


if __name__ == "__main__":
    main()
