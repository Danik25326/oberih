"""
Перетворює технічні помилки парсера Lark (LALR-термінологія, назви токенів)
на зрозумілі повідомлення з показом рядка коду і стрілкою на місце помилки.

Словник "технічна назва токена -> символ" будується АВТОМАТИЧНО з граматики
(через lark.lexer.PatternStr), а не вписується вручну - це гарантує, що
повідомлення завжди відповідають реальній граматиці, навіть якщо вона
зміниться.
"""

from lark.exceptions import UnexpectedToken, UnexpectedCharacters
from lark.lexer import PatternStr

# Для термінів, що не є фіксованим рядком (регулярні вирази) - людяні назви.
_REGEX_TERMINAL_NAMES = {
    "NAME": "ім'я (ідентифікатор)",
    "STRING": "рядок у лапках",
    "SIGNED_NUMBER": "число",
    "BOOL": "true або false",
    "CMP_OP": "оператор порівняння (==, !=, <, >, <=, >=)",
    "TIME_UNIT": "одиниця часу (ms, s, m)",
    "$END": "кінець файлу",
}


def _build_terminal_display(parser):
    display = {}
    for term in parser.terminals:
        if isinstance(term.pattern, PatternStr):
            display[term.name] = f"'{term.pattern.value}'"
        else:
            display[term.name] = _REGEX_TERMINAL_NAMES.get(term.name, term.name)
    return display


def _show_source_line(source, line, column):
    lines = source.splitlines()
    if not (1 <= line <= len(lines)):
        return ""
    code_line = lines[line - 1]
    pointer = " " * (column - 1) + "^"
    return f"\n    {code_line}\n    {pointer}\n"


def format_parse_error(exc, source, parser):
    """Повертає людяне багаторядкове повідомлення про синтаксичну помилку."""
    terminal_display = _build_terminal_display(parser)

    if isinstance(exc, UnexpectedToken):
        line, column = exc.line, exc.column
        expected_names = sorted(exc.accepts or exc.expected or [])

        if exc.token.type == "$END":
            msg = f"Синтаксична помилка: файл закінчився неочікувано (рядок {line})."
            msg += "\nЙмовірна причина: забракло закриваючої дужки '}' або ')' десь вище."
            return msg

        got = terminal_display.get(exc.token.type, exc.token.type)
        expected = [terminal_display.get(n, n) for n in expected_names]

        msg = f"Синтаксична помилка в рядку {line}, колонці {column}:"
        msg += _show_source_line(source, line, column)
        msg += f"Знайдено {got}, "
        if len(expected) > 6:
            msg += "а це тут недоречно (очікувалось щось інше на цьому місці)."
        elif expected:
            msg += f"а очікувалось одне з: {', '.join(expected)}."
        else:
            msg += "але це тут недоречно."
        return msg

    if isinstance(exc, UnexpectedCharacters):
        line, column = exc.line, exc.column
        char = source[exc.pos_in_stream] if exc.pos_in_stream < len(source) else "?"
        msg = f"Синтаксична помилка в рядку {line}, колонці {column}:"
        msg += _show_source_line(source, line, column)
        msg += f"Незрозумілий символ '{char}' - тут очікувався інший синтаксис."
        return msg

    # Загальний фолбек для інших підкласів UnexpectedInput
    return f"Синтаксична помилка: {exc}"
