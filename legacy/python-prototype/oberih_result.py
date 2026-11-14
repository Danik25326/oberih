"""
OberihResult — справжній Result<T,E> тип для рантайму Oberih.

Замість Python exceptions під капотом:
  - Ok(value)  -> OberihResult.ok(value)
  - Err(error) -> OberihResult.err(error)
  - ?          -> розпаковує Ok або пробрасує Err вгору по стеку (через EarlyReturn)
  - match      -> pattern matching на варіантах

Це перший клас рантайму — не dict, не exception, а окремий тип.
"""


class OberihResult:
    __slots__ = ("_is_ok", "_value")

    def __init__(self, is_ok: bool, value):
        self._is_ok = is_ok
        self._value = value

    # --- конструктори ---

    @staticmethod
    def ok(value):
        return OberihResult(True, value)

    @staticmethod
    def err(value):
        return OberihResult(False, value)

    # --- перевірки ---

    def is_ok(self):
        return self._is_ok

    def is_err(self):
        return not self._is_ok

    def unwrap(self):
        if self._is_ok:
            return self._value
        raise OberihUnwrapError(
            f"unwrap() викликано на Err({self._value!r})"
        )

    def unwrap_err(self):
        if not self._is_ok:
            return self._value
        raise OberihUnwrapError(
            f"unwrap_err() викликано на Ok({self._value!r})"
        )

    def value(self):
        """Внутрішнє значення без перевірки — тільки для match/pattern."""
        return self._value

    # --- для match pattern matching ---

    @property
    def variant(self):
        return "Ok" if self._is_ok else "Err"

    # --- repr ---

    def __repr__(self):
        if self._is_ok:
            return f"Ok({self._value!r})"
        return f"Err({self._value!r})"

    def __eq__(self, other):
        if isinstance(other, OberihResult):
            return self._is_ok == other._is_ok and self._value == other._value
        return NotImplemented


class OberihUnwrapError(RuntimeError):
    """Кидається коли unwrap() або unwrap_err() викликано на неправильному варіанті."""
    pass


class PropagateErr(Exception):
    """
    Внутрішній механізм оператора ?

    Коли expr? зустрічає Err — кидає PropagateErr з цим результатом.
    Найближча resilient fn або fn з Result-поверненням ловить це
    і повертає Err далі. Якщо ніхто не ловить — це незахоплена помилка.
    """
    def __init__(self, result: OberihResult):
        assert result.is_err(), "PropagateErr тільки для Err"
        self.result = result
        super().__init__(repr(result))
