#!/usr/bin/env bash
# Необязательная проверка I12 на реальных списках (нужна сеть): EasyList+EasyPrivacy, страницы с рекламой/трекерами,
# счётчик «заблокировано» в тулбаре должен вырасти. Смотрите out/gui-adblock-real/*.png.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-adblock-real
NRB="${1:-target/debug/nrb}"
P="$(mktemp -d)"
trap 'kill $PID $XEPHYR 2>/dev/null; rm -rf "$P"' EXIT
"$NRB" adblock update --profile "$P" > "$OUT/update.log" 2>&1 || { echo "пропуск: списки не скачались (нет сети?)"; exit 0; }
for site in https://www.cnn.com/ https://www.forbes.com/; do
  name=$(echo "$site" | sed 's|https://www\.||;s|[/.]||g')
  gui_launch "$NRB" --profile "$P" "$site"; sleep 25
  shot "$name"
  echo "$site: заголовок «$(title)»"
  kill "$PID"; wait "$PID" 2>/dev/null
done
gui_finish
