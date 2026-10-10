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

EXTRA=""
if [ "${HERMETIC:-0}" = 1 ]; then
  # Загрузка в герметичном режиме: у потока окна сети нет, файл должен прийти через рантайм туннеля. Узел — свой, на loopback
  # (поэтому --allow-private: цель сервера — loopback).
  NODEPORT=$((PORT + 1))
  "$NRB" node init --dir "$TMP/node" --public-host 127.0.0.1 --port "$NODEPORT" > /dev/null
  "$NRB" node run --dir "$TMP/node" > /dev/null 2>&1 &
  NODE=$!; trap 'kill $PID $XEPHYR $SRV $NODE 2>/dev/null; rm -rf "$TMP"' EXIT
  for _ in $(seq 1 40); do ss -tln | grep -q ":$NODEPORT " && break; sleep 0.25; done
  LINK="$("$NRB" node invite --dir "$TMP/node" окно 2>/dev/null)"
  EXTRA="--hermetic --allow-private --node $LINK"
  echo "режим: герметичный, через свой узел"
fi
# shellcheck disable=SC2086
XDG_CONFIG_HOME="$TMP/cfg" gui_launch "$NRB" $EXTRA --private "http://127.0.0.1:$PORT/page.html"
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
# N3b: решение принимает движок по заголовкам ответа, а не по адресу — динамический эндпоинт без расширения (имя из
# Content-Disposition, русское) и POST-форма.
click 60 300; sleep 4
check "после экспорта страница осталась на месте" "dl-page"
CSVF="$TMP/dl/отчёт.csv"
for _ in $(seq 1 20); do [ -f "$CSVF" ] && break; sleep 0.5; done
[ -f "$CSVF" ] && [ "$(cat "$CSVF")" = "$(printf 'a,b\n1,2')" ] && echo "ok   динамический эндпоинт: имя из Content-Disposition, кука учтена" \
  || { echo "FAIL экспорт не скачан: $(ls -la "$TMP/dl" 2>&1 | tr '\n' ' ')"; fail=1; }
click 60 410; sleep 4
check "после POST-загрузки страница осталась на месте" "dl-page"
POSTF="$TMP/dl/report.bin"
for _ in $(seq 1 20); do [ -f "$POSTF" ] && break; sleep 0.5; done
[ -f "$POSTF" ] && [ "$(cat "$POSTF")" = "post-report-body" ] && echo "ok   POST-загрузка принята движком" \
  || { echo "FAIL POST-загрузка не получена: $(ls -la "$TMP/dl" 2>&1 | tr '\n' ' ')"; fail=1; }
[ -z "$(ls "$TMP/dl" | grep '\.part$')" ] && echo "ok   .part не остался (все)" || { echo "FAIL остался .part"; fail=1; }
shot 02-window
gui_finish
