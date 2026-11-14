#!/usr/bin/env bash
# Повідомляє про збій нічної перевірки, щоб його не можна було не помітити:
# створює issue (або додає коментар до ВЖЕ відкритого з тією самою міткою, щоб не засмічувати).
#
# Використання: report-failure.sh <мітка> <заголовок> <файл-з-текстом>
# Потрібні: gh (є на runner'ах GitHub), GH_TOKEN, права `issues: write` у workflow.
set -euo pipefail

label="${1:?мітка}"; title="${2:?заголовок}"; body="${3:?файл з текстом}"

# Мітка може ще не існувати; помилка «вже є» не страшна.
gh label create "$label" --color B60205 --description "Автоматично: збій нічної перевірки" 2>/dev/null || true

existing="$(gh issue list --label "$label" --state open --json number --jq '.[0].number // empty')"
if [ -n "$existing" ]; then
  gh issue comment "$existing" --body-file "$body"
  echo "Додано коментар до відкритого issue #$existing"
else
  gh issue create --title "$title" --label "$label" --body-file "$body"
fi
