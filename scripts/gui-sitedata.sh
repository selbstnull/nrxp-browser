#!/usr/bin/env bash
# GUI-сценарий N4a: окно «Данные сайтов». Страница ставит куку и localStorage и показывает в заголовке, что у неё уже есть.
# Первый визит — ничего нет; второй — есть оба; «Очистить всё» в окне (Ctrl+Shift+Del) — и после перезагрузки снова ничего.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-sitedata
NRB="${1:-target/debug/nrb}"
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT
python3 tests/fixtures/sitedata_server.py > "$TMP/port" &
SRV=$!
for _ in $(seq 1 30); do [ -s "$TMP/port" ] && break; sleep 0.2; done
URL="http://127.0.0.1:$(head -1 "$TMP/port")/"
gui_launch "$NRB" --private "$URL"; sleep 4
check_wait "первый визит: ни куки, ни localStorage" "cookie=false ls=false"
xdotool key F5; sleep 3
check_wait "второй визит: есть и кука, и localStorage" "cookie=true ls=true"
xdotool key ctrl+shift+Delete; sleep 1.5; shot 01-window
click 424 66; sleep 1.5; shot 02-cleared      # «Очистить всё» (координаты — по скриншоту окна)
xdotool key Escape; sleep 0.5
xdotool key F5; sleep 3
check_wait "после очистки страница снова ничего не помнит" "cookie=false ls=false"
gui_finish
