#!/usr/bin/env bash
# GUI-сценарий I12: пользовательские скрипты и стили. Каталог userscripts профиля: скрипт для 127.0.0.1 (должен сработать),
# скрипт для чужого адреса (не должен), скрипт с @exclude (не должен), скрипт без @match (не должен), стиль для 127.0.0.1.
# Итог страница пишет в заголовок: «<us>|<other>|<цвет h1>».
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-userscripts
NRB="${1:-target/debug/nrb}"
PORT="${NRB_E2E_PORT:-18990}"
P="$(mktemp -d)"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory tests/fixtures/userscripts > /dev/null 2>&1 &
WEB=$!
trap 'kill $PID $XEPHYR $WEB 2>/dev/null; rm -rf "$P"' EXIT
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && break; sleep 0.1; done
mkdir -p "$P/userscripts"
cat > "$P/userscripts/mine.user.js" <<JS
// ==UserScript==
// @name Мой скрипт
// @match http://127.0.0.1:*/*
// ==/UserScript==
document.body.dataset.us = 'ran';
JS
cat > "$P/userscripts/foreign.user.js" <<JS
// ==UserScript==
// @match *://example.org/*
// ==/UserScript==
document.body.dataset.other = 'foreign-ran';
JS
cat > "$P/userscripts/excluded.user.js" <<JS
// ==UserScript==
// @match http://127.0.0.1:*/*
// @exclude http://127.0.0.1:*/page.html
// ==/UserScript==
document.body.dataset.other = 'excluded-ran';
JS
cat > "$P/userscripts/nomatch.user.js" <<JS
document.body.dataset.other = 'nomatch-ran';
JS
cat > "$P/userscripts/color.user.css" <<CSS
/* ==UserStyle==
   @match http://127.0.0.1:*/*
   ==/UserStyle== */
h1 { color: rgb(255, 0, 0) !important; }
CSS

gui_launch "$NRB" --profile "$P" "http://127.0.0.1:$PORT/page.html"; sleep 5
check "скрипт для этого адреса сработал, чужие/исключённые/без @match — нет, стиль применён" "ran|нет|rgb(255, 0, 0)"; shot 01-page
grep -q "nomatch.user.js: нет @match" "$OUT/nrb.log" && echo "ok   скрипт без @match назван в предупреждении" || { echo "FAIL нет предупреждения про @match"; fail=1; }
gui_finish
