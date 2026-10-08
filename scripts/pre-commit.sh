#!/usr/bin/env bash
# Хук pre-commit: не пускает в коммит данные времени выполнения (sqlite, bottles/, профили) и файлы крупнее 2 МБ.
# Установка: scripts/pre-commit.sh --install
set -eu
if [ "${1:-}" = "--install" ]; then
  ln -sf ../../scripts/pre-commit.sh "$(git rev-parse --git-dir)/hooks/pre-commit"; echo "хук установлен"; exit 0
fi
bad=0
while IFS= read -r f; do
  case "$f" in
    bottles/*|*.sqlite|*.sqlite-wal|*.sqlite-shm|out/*|target*/*) echo "в коммит не должно попадать: $f"; bad=1;;
  esac
  if [ -f "$f" ] && [ "$(stat -c %s "$f")" -gt 2097152 ]; then echo "слишком большой файл (>2 МБ): $f"; bad=1; fi
done < <(git diff --cached --name-only --diff-filter=AM)
exit "$bad"
