#!/usr/bin/env bash
# GUI-сценарий N3a: загрузка файла по ссылке. Страница ставит куку, файл отдаётся только с кукой — значит, загрузчик обязан взять
# куки из хранилища браузера. Проверяем: страница осталась на месте (ответ 204), файл появился в каталоге загрузок с верным
# содержимым, окно «Загрузки» показало «Готово».
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-downloads
NRB="${1:-target/debug/nrb}"
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/cfg" "$TMP/dl"
echo "XDG_DOWNLOAD_DIR=\"$TMP/dl\"" > "$TMP/cfg/user-dirs.dirs"
python3 tests/fixtures/download_server.py > "$TMP/port" &
SRV=$!
for _ in $(seq 1 30); do [ -s "$TMP/port" ] && break; sleep 0.2; done
PORT=$(head -1 "$TMP/port")

XDG_CONFIG_HOME="$TMP/cfg" gui_launch "$NRB" --private "http://127.0.0.1:$PORT/page.html"
sleep 4
check "страница загрузилась" "dl-page"
click 60 100; sleep 4
shot 01-after-click
check "после клика страница осталась на месте" "dl-page"
FILE="$TMP/dl/data.zip"
for _ in $(seq 1 20); do [ -f "$FILE" ] && break; sleep 0.5; done
if [ -f "$FILE" ] && [ "$(stat -c %s "$FILE")" -eq 36000 ] && head -c 18 "$FILE" | grep -q "nrb-download-test"; then
  echo "ok   файл загружен с кукой ($(stat -c %s "$FILE") байт)"
else
  echo "FAIL файл не загружен или повреждён: $(ls -la "$TMP/dl" 2>&1 | tr '\n' ' ')"; fail=1
fi
[ -z "$(ls "$TMP/dl" | grep '\.part$')" ] && echo "ok   .part не остался" || { echo "FAIL остался .part"; fail=1; }
shot 02-window
gui_finish
