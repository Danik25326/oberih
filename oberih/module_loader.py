"""
Розв'язання import "path.obh" - об'єднує декларації з кількох файлів
в одне дерево, ще до того, як воно потрапляє в evaluator/typechecker.

Проста модель "include": усі функції/структури з імпортованих файлів
потрапляють в те саме глобальне середовище, без просторів імен. Циклічні
та повторні імпорти безпечно ігноруються (файл читається лише раз).
"""

import os
from lark import Tree
from lark.exceptions import UnexpectedInput
from friendly_errors import format_parse_error


class ObirihCompileError(Exception):
    """Несе вже відформатоване, людяне повідомлення про помилку компіляції."""
    pass


def _unescape_import_path(raw_string_token):
    return str(raw_string_token)[1:-1]


def _is_private(node):
    return (
        node.data in ("fn_decl", "struct_decl")
        and len(node.children) > 0
        and hasattr(node.children[0], "type")
        and node.children[0].type == "PRIVATE_KW"
    )


def load_program_with_imports(entry_path, parser, _visited=None, _project_root=None):
    """Повертає єдине дерево 'start', що об'єднує декларації з усіх
    файлів, досяжних через import, починаючи з entry_path.

    Кожній декларації проставляється атрибут `.origin_file` - потрібен
    для компіляторної перевірки видимості `private` (check_private_visibility
    у typechecker.py): приватну функцію можна викликати лише з коду, що
    оголошений у ТОМУ САМОМУ файлі."""
    if _visited is None:
        _visited = set()
    if _project_root is None:
        _project_root = os.path.dirname(os.path.abspath(entry_path))

    entry_path = os.path.abspath(entry_path)
    if entry_path in _visited:
        return []  # файл вже імпортовано - уникаємо циклів і дублювання
    _visited.add(entry_path)

    try:
        with open(entry_path, encoding="utf-8") as f:
            source = f.read()
    except FileNotFoundError:
        raise ObirihCompileError(f"Файл не знайдено: {entry_path}")

    try:
        tree = parser.parse(source)
    except UnexpectedInput as e:
        raise ObirihCompileError(
            f"Помилка компіляції ({entry_path}):\n{format_parse_error(e, source, parser)}"
        )

    all_decls = []
    base_dir = os.path.dirname(entry_path)

    for node in tree.children:
        if node.data == "import_stmt":
            rel_path = _unescape_import_path(node.children[0])
            import_path = _resolve_import_path(rel_path, base_dir, _project_root)
            all_decls.extend(
                load_program_with_imports(import_path, parser, _visited, _project_root)
            )
        else:
            node.origin_file = entry_path  # для перевірки видимості private
            all_decls.append(node)

    return all_decls


def _resolve_import_path(rel_path, base_dir, project_root):
    """Спочатку шукає відносно файлу, що імпортує; якщо не знайдено -
    у obh_modules/ у корені проєкту (туди `oberih.py install` кладе
    залежності)."""
    direct = os.path.join(base_dir, rel_path)
    if os.path.exists(direct):
        return direct

    via_modules = os.path.join(project_root, "obh_modules", rel_path)
    if os.path.exists(via_modules):
        return via_modules

    return direct  # немає - хай впаде з чіткою помилкою "файл не знайдено"


def parse_with_imports(entry_path, parser):
    """Точка входу: повертає Tree('start', [...]) - готовий для load_program."""
    decls = load_program_with_imports(entry_path, parser)
    return Tree("start", decls)
