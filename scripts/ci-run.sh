#!/usr/bin/env bash
# Запуск команды в CI с выводом хвоста лога как ошибки-аннотации при падении: аннотации видны по публичному API
# (`/check-runs/<id>/annotations`) без токена, а полные логи GitHub отдаёт только администратору.
# Использование: scripts/ci-run.sh "<заголовок>" команда аргументы…
set -u
title="$1"; shift
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  msg="$(sed -E 's/\x1b\[[0-9;]*m//g' "$log" | tail -n 70 | tail -c 9000)"
  msg="${msg//'%'/'%25'}"; msg="${msg//$'\r'/'%0D'}"; msg="${msg//$'\n'/'%0A'}"
  echo "::error title=${title} (код ${status})::${msg}"
fi
exit "$status"
