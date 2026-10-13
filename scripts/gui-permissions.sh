#!/usr/bin/env bash
# GUI-сценарий N4b: запомненные решения о разрешениях. Страница по http (запоминаются только http/https), профиль в каталоге.
# 1) запрос уведомлений → «Запомнить» + «Разрешить» → granted; 2) новый запуск с тем же профилем → окна нет, сразу granted.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-permissions
NRB="${1:-target/debug/nrb}"
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT
PORT=$((20000 + RANDOM % 20000))
(cd tests/fixtures && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SRV=$!
for _ in $(seq 1 30); do ss -tln | grep -q ":$PORT " && break; sleep 0.2; done
URL="http://127.0.0.1:$PORT/dialogs.html"
TOP=${TOOLBAR_PX:-85}
REMEMBER_X=${REMEMBER_X:-484}; REMEMBER_Y=${REMEMBER_Y:-158}; ALLOW_X=${ALLOW_X:-514}; ALLOW_Y=${ALLOW_Y:-179}

gui_launch "$NRB" --profile "$TMP/profile" "$URL"; sleep 3
check "страница загружена" "dialogs: ready"
click 120 $((TOP + 315)); sleep 1.5; shot 01-ask
click "$REMEMBER_X" "$REMEMBER_Y"; sleep 0.5; shot 02-remember
click "$ALLOW_X" "$ALLOW_Y"; sleep 1.5
check_wait "первый раз: разрешено в окне" "notif: granted"
kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1

gui_launch "$NRB" --profile "$TMP/profile" "$URL"; sleep 3
check "вторая сессия: страница загружена" "dialogs: ready"
click 120 $((TOP + 315)); sleep 2; shot 03-second
check_wait "вторая сессия: решение вспомнено без окна" "notif: granted"
gui_finish
