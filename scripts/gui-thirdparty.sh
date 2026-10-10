#!/usr/bin/env bash
# GUI-сценарий N4b: сторонние куки. Сайт (localhost) подключает картинку трекера (127.0.0.1 — другой сайт), трекер ставит куку и
# пишет в журнал заголовок Cookie каждого запроса. Без настройки второй запрос несёт куку трекера; с `block_third_party_cookies`
# (settings.toml профиля) — не несёт. Журнал читаем с диска, страница ничего не показывает.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-thirdparty
NRB="${1:-target/debug/nrb}"
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT

run_case() { # имя, содержимое settings.toml
  local name="$1" settings="$2" log="$TMP/$1.log"
  mkdir -p "$TMP/$name-profile"; printf '%s\n' "$settings" > "$TMP/$name-profile/settings.toml"
  : > "$log"
  python3 tests/fixtures/thirdparty_server.py "$log" > "$TMP/$name.ports" &
  SRV=$!
  for _ in $(seq 1 30); do [ -s "$TMP/$name.ports" ] && break; sleep 0.2; done
  local site; site=$(awk '{print $1}' "$TMP/$name.ports")
  gui_launch "$NRB" --profile "$TMP/$name-profile" "http://localhost:$site/"; sleep 4
  check "$name: страница сайта загрузилась" "tp-site"
  xdotool key F5; sleep 3
  for _ in $(seq 1 20); do [ "$(wc -l < "$log")" -ge 2 ] && break; sleep 0.5; done
  kill $PID $SRV 2>/dev/null; wait $PID $SRV 2>/dev/null
  CASE_LOG="$log"
}

run_case default 'adblock = false'
if [ "$(sed -n 2p "$CASE_LOG")" = "cookie=t=1" ]; then echo "ok   по умолчанию: куку трекера видно во втором запросе"
else echo "FAIL по умолчанию: журнал трекера: $(tr '\n' ' ' < "$CASE_LOG")"; fail=1; fi

run_case blocked $'adblock = false\nblock_third_party_cookies = true'
if [ "$(wc -l < "$CASE_LOG")" -ge 2 ] && ! grep -q "t=1" "$CASE_LOG"; then echo "ok   с блокировкой: трекер кук не получил ($(tr '\n' ' ' < "$CASE_LOG"))"
else echo "FAIL с блокировкой: журнал трекера: $(tr '\n' ' ' < "$CASE_LOG")"; fail=1; fi
gui_finish
