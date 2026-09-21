"""
Мінімальний пакетний менеджер Oberih.

Немає централізованого реєстру пакетів (як npm registry чи PyPI) - це
реалістично для мови на цьому етапі. Натомість залежності вказуються прямо
як джерело: локальний шлях або Git-репозиторій. `oberih.py install`
завантажує їх у obh_modules/, звідки їх видно системі import.

Формат маніфесту (oberih.json):
{
  "name": "my-project",
  "version": "0.1.0",
  "dependencies": {
    "mathutils": "./path/to/local/package",
    "somelib": "https://github.com/user/somelib.git"
  }
}
"""

import json
import os
import shutil
import subprocess
import sys

MANIFEST_NAME = "oberih.json"
MODULES_DIR = "obh_modules"


def _is_git_source(source):
    return source.startswith("http://") or source.startswith("https://") or source.endswith(".git")


def load_manifest(project_dir):
    manifest_path = os.path.join(project_dir, MANIFEST_NAME)
    if not os.path.exists(manifest_path):
        print(f"Немає {MANIFEST_NAME} в {project_dir}", file=sys.stderr)
        sys.exit(1)
    with open(manifest_path, encoding="utf-8") as f:
        return json.load(f)


def install_dependency(name, source, project_dir):
    target_dir = os.path.join(project_dir, MODULES_DIR, name)

    if os.path.exists(target_dir):
        shutil.rmtree(target_dir)
    os.makedirs(os.path.dirname(target_dir), exist_ok=True)

    if _is_git_source(source):
        print(f"  {name}: клонування з {source} ...")
        result = subprocess.run(
            ["git", "clone", "--depth", "1", source, target_dir],
            capture_output=True, text=True,
        )
        if result.returncode != 0:
            print(f"  {name}: ПОМИЛКА клонування:\n{result.stderr}", file=sys.stderr)
            return False
        print(f"  {name}: встановлено з Git у {MODULES_DIR}/{name}")
        return True
    else:
        source_path = os.path.abspath(os.path.join(project_dir, source))
        if not os.path.exists(source_path):
            print(f"  {name}: локальний шлях не знайдено: {source_path}", file=sys.stderr)
            return False
        shutil.copytree(source_path, target_dir)
        print(f"  {name}: скопійовано з {source} у {MODULES_DIR}/{name}")
        return True


def install_all(project_dir="."):
    manifest = load_manifest(project_dir)
    deps = manifest.get("dependencies", {})
    if not deps:
        print("Немає залежностей у маніфесті.")
        return

    print(f"Встановлення {len(deps)} залежностей для '{manifest.get('name', '?')}':")
    ok = True
    for name, source in deps.items():
        if not install_dependency(name, source, project_dir):
            ok = False

    if ok:
        print(f"\nГотово. Залежності лежать у {MODULES_DIR}/.")
    else:
        print("\nЗавершено з помилками - див. вище.", file=sys.stderr)
        sys.exit(1)
