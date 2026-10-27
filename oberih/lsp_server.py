#!/usr/bin/env python3
"""
Мінімальний LSP-сервер для Oberih.

Реалізує:
- initialize / initialized (handshake)
- textDocument/didOpen, didChange, didSave -> публікує РЕАЛЬНУ діагностику
  (ті самі помилки, що й наш компілятор: синтаксис, обов'язковий
  deadline/retryBudget, типи, заборона I/O в emergencyFallback, обов'язковий
  budget для agent.call)
- textDocument/completion -> список ключових слів/модифікаторів/вбудованих
  об'єктів/типів мови

Чесно НЕ реалізує: go-to-definition, hover, rename. Помилки typechecker
(на відміну від синтаксичних) прив'язані до рядка 1, бо наразі не несуть
власних координат у AST - це задокументоване обмеження, а не баг.

Протокол: JSON-RPC over stdio з Content-Length фреймінгом (стандарт LSP).
Без зовнішніх залежностей окрім lark (яка вже потрібна самій мові).
"""

import sys
import json
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from lark import Lark
from lark.exceptions import UnexpectedToken, UnexpectedCharacters, UnexpectedInput
from evaluator import load_program
import evaluator
from typechecker import (
    check_program, type_check_program, check_emergency_fallback_no_io,
    check_agent_budget_required,
)

KEYWORDS = ["fn", "resilient", "struct", "let", "return", "if", "else",
            "while", "for", "in", "import", "true", "false"]
MODIFIERS = ["deadline", "retryBudget", "retries", "timeout", "fallback",
             "emergencyFallback", "circuitBreaker", "idempotent", "cache",
             "rateLimit", "bulkhead", "hedging", "budget", "durable", "traced"]
BUILTIN_OBJECTS = ["http", "paymentGateway", "weatherApi", "llm", "agent"]
BUILTIN_FUNCTIONS = ["print", "len"]
TYPES = ["Number", "String", "Boolean", "Result", "Option", "List", "Map"]

_GRAMMAR_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "grammar.lark")
with open(_GRAMMAR_PATH, encoding="utf-8") as f:
    _GRAMMAR = f.read()

_parser = Lark(_GRAMMAR, parser="lalr", propagate_positions=True)
_documents = {}  # uri -> поточний текст


# --- JSON-RPC фреймінг (Content-Length, як того вимагає специфікація LSP) ---

def _read_message():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("utf-8").rstrip("\r\n")
        if line == "":
            break
        if ":" in line:
            key, _, value = line.partition(":")
            headers[key.strip()] = value.strip()
    length = int(headers.get("Content-Length", 0))
    if length == 0:
        return None
    body = sys.stdin.buffer.read(length).decode("utf-8")
    return json.loads(body)


def _write_message(obj):
    body = json.dumps(obj)
    data = body.encode("utf-8")
    header = f"Content-Length: {len(data)}\r\n\r\n".encode("utf-8")
    sys.stdout.buffer.write(header + data)
    sys.stdout.buffer.flush()


def _send_response(id_, result):
    _write_message({"jsonrpc": "2.0", "id": id_, "result": result})


def _send_notification(method, params):
    _write_message({"jsonrpc": "2.0", "method": method, "params": params})


# --- Діагностика: перевикористовує РЕАЛЬНИЙ компілятор мови ---

def _diag_from_unexpected(e, prefix):
    line = max(e.line - 1, 0)
    col = max(e.column - 1, 0)
    return {
        "range": {
            "start": {"line": line, "character": col},
            "end": {"line": line, "character": col + 1},
        },
        "severity": 1,  # Error
        "message": f"{prefix}: {e}",
        "source": "oberih",
    }


def _diag_at_line_one(message):
    return {
        "range": {
            "start": {"line": 0, "character": 0},
            "end": {"line": 0, "character": 1},
        },
        "severity": 1,
        "message": message,
        "source": "oberih",
    }


def diagnostics_for_text(text):
    """Повертає список LSP-діагностик для тексту .obh файлу.
    Винесено окремою функцією, щоб її можна було тестувати напряму,
    без підняття цілого stdio-сервера."""
    try:
        tree = _parser.parse(text)
    except UnexpectedToken as e:
        return [_diag_from_unexpected(e, "Синтаксична помилка")]
    except UnexpectedCharacters as e:
        return [_diag_from_unexpected(e, "Незрозумілий символ")]
    except UnexpectedInput as e:
        return [_diag_at_line_one(f"Синтаксична помилка: {e}")]

    try:
        load_program(tree)
        errors = (
            check_program(evaluator.REGISTRY)
            + type_check_program(evaluator.REGISTRY, evaluator.STRUCTS)
            + check_emergency_fallback_no_io(evaluator.REGISTRY)
            + check_agent_budget_required(evaluator.REGISTRY)
        )
        return [_diag_at_line_one(msg) for msg in errors]
    except Exception as e:
        return [_diag_at_line_one(f"Внутрішня помилка перевірки: {e}")]


def _publish_diagnostics(uri, text):
    _send_notification("textDocument/publishDiagnostics", {
        "uri": uri,
        "diagnostics": diagnostics_for_text(text),
    })


def _completion_items():
    items = []
    for kw in KEYWORDS:
        items.append({"label": kw, "kind": 14})       # Keyword
    for mod in MODIFIERS:
        items.append({"label": mod, "kind": 3})        # Function
    for obj in BUILTIN_OBJECTS:
        items.append({"label": obj, "kind": 6})        # Variable
    for fn in BUILTIN_FUNCTIONS:
        items.append({"label": fn, "kind": 3})
    for t in TYPES:
        items.append({"label": t, "kind": 7})          # Class
    return items


def main():
    while True:
        msg = _read_message()
        if msg is None:
            break

        method = msg.get("method")
        params = msg.get("params", {}) or {}
        msg_id = msg.get("id")

        if method == "initialize":
            _send_response(msg_id, {
                "capabilities": {
                    "textDocumentSync": 1,  # Full sync
                    "completionProvider": {"triggerCharacters": ["."]},
                }
            })
        elif method == "initialized":
            pass
        elif method == "textDocument/didOpen":
            uri = params["textDocument"]["uri"]
            text = params["textDocument"]["text"]
            _documents[uri] = text
            _publish_diagnostics(uri, text)
        elif method == "textDocument/didChange":
            uri = params["textDocument"]["uri"]
            text = params["contentChanges"][0]["text"]
            _documents[uri] = text
            _publish_diagnostics(uri, text)
        elif method == "textDocument/didSave":
            uri = params["textDocument"]["uri"]
            if uri in _documents:
                _publish_diagnostics(uri, _documents[uri])
        elif method == "textDocument/completion":
            _send_response(msg_id, _completion_items())
        elif method == "shutdown":
            _send_response(msg_id, None)
        elif method == "exit":
            break
        elif msg_id is not None:
            _send_response(msg_id, None)  # невідомий запит - не зависати


if __name__ == "__main__":
    main()
