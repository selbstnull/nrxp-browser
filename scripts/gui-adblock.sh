#!/usr/bin/env bash
# GUI-сценарий I12: блокировщик рекламы. Локальный HTTP-сервер отдаёт страницу со скриптом из /ads/ и «чистым» скриптом;
# правило `/ads/*$script` из каталога filters профиля должно срезать только первый. Контроль: без списка скрипт грузится;
# с `adblock = false` в settings.toml — тоже грузится.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-adblock
NRB="${1:-target/debug/nrb}"
PORT="${NRB_E2E_PORT:-18980}"
P1="$(mktemp -d)"; P2="$(mktemp -d)"; P3="$(mktemp -d)"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory tests/fixtures/adblock > /dev/null 2>&1 &
WEB=$!
trap 'kill $PID $XEPHYR $WEB 2>/dev/null; rm -rf "$P1" "$P2" "$P3"' EXIT
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && break; sleep 0.1; done
URL="http://127.0.0.1:$PORT/page.html"
mkdir -p "$P1/filters"; printf '! тест\n/ads/*$script\n' > "$P1/filters/test.txt"

echo "== со списком: скрипт из /ads/ заблокирован"
gui_launch "$NRB" --profile "$P1" "$URL"; sleep 5
check "рекламный скрипт не выполнился (onerror)" "blocked"; shot 01-blocked
kill "$PID"; wait "$PID" 2>/dev/null

echo "== без списка: тот же скрипт грузится"
gui_launch "$NRB" --profile "$P2" "$URL"; sleep 5
check "без правил скрипт выполнился" "ad-loaded"
kill "$PID"; wait "$PID" 2>/dev/null

echo "== список есть, но adblock = false"
mkdir -p "$P3/filters"; cp "$P1/filters/test.txt" "$P3/filters/"
printf 'adblock = false\n' > "$P3/settings.toml"
gui_launch "$NRB" --profile "$P3" "$URL"; sleep 5
check "при выключенной настройке скрипт выполнился" "ad-loaded"
gui_finish
