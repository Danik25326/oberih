"""
OberihSpawnHandle — справжній handle для spawn з підтримкою скасування.

Проблеми старої реалізації:
  - .join() без timeout — висить нескінченно
  - немає cancel() — "програшна" спроба hedging добігає до кінця
  - немає propagation помилок як OberihResult

Що тут:
  - join(timeout?) — чекає результат, опціонально з дедлайном
  - cancel() — сигналізує функції що треба зупинитись (через CancelToken)
  - isDone() — неблокуючий статус
  - результат або значення, або OberihResult.err(...)

CancelToken — легкий механізм кооперативного скасування.
Функція може перевіряти token.is_cancelled() і зупинитись достроково.
Python не дозволяє примусово вбити thread, тому скасування кооперативне —
але для мережевих викликів (де це найважливіше) interpreter.py вже
використовує future.cancel() і timeout, тому на практиці це працює.
"""

import concurrent.futures
import threading
from oberih_result import OberihResult


class CancelToken:
    """Передається у spawned-функцію як спосіб дізнатись про скасування."""

    def __init__(self):
        self._event = threading.Event()

    def cancel(self):
        self._event.set()

    def is_cancelled(self) -> bool:
        return self._event.is_set()

    def __repr__(self):
        return f"CancelToken(cancelled={self.is_cancelled()})"


class OberihSpawnHandle:
    """
    Handle повернутий spawn.

    Поля:
      _future   — concurrent.futures.Future з результатом
      _token    — CancelToken переданий у функцію
      _fn_name  — ім'я функції (для repr і помилок)
    """

    def __init__(self, future: concurrent.futures.Future, token: CancelToken, fn_name: str):
        self._future = future
        self._token = token
        self._fn_name = fn_name

    def join(self, timeout=None):
        """
        Чекає завершення і повертає результат.

        timeout: секунди (float) або None (чекати нескінченно).
        Якщо timeout спливає — повертає OberihResult.err("timeout").
        Якщо функція кинула виняток — OberihResult.err(str(exception)).
        """
        try:
            result = self._future.result(timeout=timeout)
            return result
        except concurrent.futures.TimeoutError:
            return OberihResult.err(f"spawn timeout: {self._fn_name}")
        except concurrent.futures.CancelledError:
            return OberihResult.err(f"spawn cancelled: {self._fn_name}")
        except Exception as e:
            return OberihResult.err(str(e))

    def cancel(self):
        """
        Сигналізує функції про скасування (кооперативно через CancelToken)
        і намагається скасувати Future якщо ще не почалась.
        Повертає True якщо Future вдалось скасувати до старту.
        """
        self._token.cancel()
        return self._future.cancel()

    def is_done(self) -> bool:
        return self._future.done()

    def is_cancelled(self) -> bool:
        return self._future.cancelled()

    def __repr__(self):
        state = "done" if self._future.done() else "running"
        return f"SpawnHandle({self._fn_name}, {state})"
