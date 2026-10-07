#!/usr/bin/env bash
# GUI-сценарий I6: профиль на диске — сессия вкладок, закладки, журнал и подсказки в адресной строке.
# Первый запуск открывает две локальные страницы; процесс убивается (SIGTERM) — сессия должна быть уже сохранена
# (сохранение идёт по ходу работы, не только при выходе). Второй запуск с тем же профилем и без URL восстанавливает вкладки.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-profile
NRB="${1:-target/debug/nrb}"
PROFILE="$(mktemp -d)"
trap 'kill $PID $XEPHYR 2>/dev/null; rm -rf "$PROFILE"' EXIT
A="file://$PWD/tests/fixtures/nav-a.html"
B="file://$PWD/tests/fixtures/nav-b.html"

echo "== запуск 1: две вкладки, закладка"
gui_launch "$NRB" --profile "$PROFILE" "$A" "$B"
sleep 3
check "первая вкладка активна после открытия двух" "Page A"
xdotool key ctrl+d; sleep 1; shot 01-bookmarked
xdotool key ctrl+Tab; sleep 1
check "вторая вкладка" "Page B"
sleep 2   # дебаунс сохранения сессии (1 с)
kill "$PID"; wait "$PID" 2>/dev/null

echo "== запуск 2: тот же профиль, без URL"
gui_launch "$NRB" --profile "$PROFILE"
sleep 3
check "восстановлена активная вкладка (Page B)" "Page B"
xdotool key ctrl+Tab; sleep 1
check "восстановлена вторая вкладка (Page A)" "Page A"

echo "== подсказки из журнала"
xdotool key ctrl+l; sleep 0.3; xdotool type --delay 60 "nav-b"; sleep 1; shot 02-suggest
xdotool mousemove --window "$WID" 400 70; sleep 0.3; xdotool click 1; sleep 2
check "клик по подсказке открыл страницу" "Page B"

echo "== окна журнала и закладок"
xdotool key ctrl+h; sleep 1; shot 03-history; xdotool key Escape; sleep 0.5
xdotool key ctrl+shift+o; sleep 1; shot 04-bookmarks; xdotool key Escape; sleep 0.5

echo "== файлы профиля"
for f in profile.sqlite session.json; do [ -s "$PROFILE/$f" ] && echo "ok   $f есть" || { echo "FAIL нет $f"; fail=1; }; done
gui_finish
