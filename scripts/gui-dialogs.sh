#!/usr/bin/env bash
# GUI-сценарий I2.1: диалоги страницы (alert/confirm/prompt), выпадающий список, запрос разрешения.
# Страница-фикстура tests/fixtures/dialogs.html, кнопки в фиксированных позициях. Скриншоты — out/gui-dialogs/.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-dialogs
NRB="${1:-target/debug/nrb}"
# shellcheck disable=SC2086
gui_launch "$NRB" ${NRB_ARGS:-} "file://$PWD/tests/fixtures/dialogs.html"
sleep 3
check "страница загружена" "dialogs: ready"
TOP=${TOOLBAR_PX:-85}   # высота тулбара в пикселях окна (без HiDPI)

click 120 $((TOP + 35)); sleep 1; shot 01-alert
xdotool key Return; sleep 1
check "alert: после OK страница продолжила" "alert: done"

click 120 $((TOP + 105)); sleep 1; shot 02-confirm
xdotool key Return; sleep 1
check "confirm: OK → true" "confirm: true"
click 120 $((TOP + 105)); sleep 1
xdotool key Escape; sleep 1
check "confirm: Esc → false" "confirm: false"

click 120 $((TOP + 175)); sleep 1; shot 03-prompt
xdotool key ctrl+a; xdotool type --delay 60 "xyz"; sleep 0.3; xdotool key Return; sleep 1
check "prompt: введённый текст" "prompt: xyz"

click 120 $((TOP + 245)); sleep 1; shot 04-select
click 70 $((TOP + 300)); sleep 1
check "select: выбран вариант" "select: "

# Окно разрешений: Notification.requestPermission() (в Servo включено prefs `dom_notification_enabled`).
click 120 $((TOP + 315)); sleep 1.5; shot 05-permission
xdotool key Escape; sleep 1.5
check "разрешение (Notification): Esc → отказ" "notif: denied"

# HTTP Basic-авторизация: локальный сервер, окно логина/пароля.
python3 tests/fixtures/auth_server.py > "$OUT/auth-port.txt" &
AUTH_PID=$!
trap 'kill $PID $XEPHYR $AUTH_PID 2>/dev/null' EXIT
for _ in $(seq 1 20); do [ -s "$OUT/auth-port.txt" ] && break; sleep 0.2; done
AUTH_PORT=$(head -1 "$OUT/auth-port.txt")
xdotool key ctrl+l; sleep 0.3; xdotool type --delay 30 "127.0.0.1:$AUTH_PORT"; xdotool key Return; sleep 2; shot 06-auth
xdotool type --delay 50 "user"; xdotool key Tab; xdotool type --delay 50 "passwd"; sleep 0.3; xdotool key Return; sleep 2
check_wait "HTTP-авторизация: логин и пароль приняты" "auth ok"
gui_finish
