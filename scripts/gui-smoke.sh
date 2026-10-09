#!/usr/bin/env bash
# GUI-смоук I2: запускает окно nrb на $DISPLAY и проходит сценарий через xdotool, проверяя заголовок окна.
# Требует X11 (xdotool, imagemagick `import`). Сеть нужна (используются реальные сайты, кроме локальной фикстуры).
# Использование: scripts/gui-smoke.sh [путь/к/nrb]   Скриншоты — в out/gui-smoke/.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-smoke
NRB="${1:-target/debug/nrb}"
FIXTURE="file://$PWD/tests/fixtures/input.html"
# NRB_ARGS — доп. аргументы nrb, например "--node-addr 127.0.0.1:18443" (весь трафик движка через туннель).
# shellcheck disable=SC2086
gui_launch "$NRB" ${NRB_ARGS:-} "https://example.org"
sleep 4
check "стартовая страница" "Example Domain"
shot 01-start

xdotool key ctrl+l; sleep 0.3; xdotool type --delay 40 "news.ycombinator.com"; xdotool key Return; sleep 6
check_wait "омнибокс: ввод домена" "Hacker News"
shot 02-hn

# Клик по ссылке и «назад» — на локальных фикстурах (детерминированно, без сети).
xdotool key ctrl+l; sleep 0.3; xdotool type --delay 30 "file://$PWD/tests/fixtures/nav-a.html"; xdotool key Return; sleep 2
check "локальная страница A" "Page A"
xdotool mousemove --window "$WID" 200 150; sleep 0.3; xdotool click 1; sleep 2
check "клик по ссылке" "Page B"
xdotool key alt+Left; sleep 3.5
check "назад (Alt+←)" "Page A"
xdotool key alt+Right; sleep 3.5
check "вперёд (Alt+→)" "Page B"

xdotool key ctrl+t; sleep 0.8; xdotool type --delay 40 "rust lang"; xdotool key Return; sleep 8
check "новая вкладка + поиск" "DuckDuckGo"
shot 03-search
xdotool key ctrl+Tab; sleep 1.5
check "переключение вкладок (Ctrl+Tab)" "Page B"

xdotool key ctrl+t; sleep 0.8; xdotool type --delay 30 "$FIXTURE"; xdotool key Return; sleep 3
xdotool mousemove --window "$WID" 300 156; xdotool click 1; sleep 0.5
xdotool type --delay 80 "abc "; sleep 0.5; xdotool type --delay 250 "привет"; sleep 1
# xdotool перекартирует клавиатуру на лету и иногда дублирует первый символ кириллицы — проверяем хвост слова
check "ввод в поле страницы (ASCII + кириллица)" "ривет"
shot 04-input

xdotool key ctrl+w; sleep 1
gui_finish
