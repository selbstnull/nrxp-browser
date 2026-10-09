#!/usr/bin/env bash
# GUI-сценарий N5a: поиск по странице (Ctrl+F). Страница фикстуры сама пишет число меток и номер активной в заголовок.
# Видимых совпадений «alpha» без учёта регистра: Alpha, alpha, ALPHA, alpha (в конце) = 4; скрытый абзац и скрипт не считаются.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-find
NRB="${1:-target/debug/nrb}"
gui_launch "$NRB" --private "file://$PWD/tests/fixtures/find.html"; sleep 3
check "страница загрузилась" "find:"
xdotool key ctrl+f; sleep 0.7
xdotool type --delay 60 "alpha"; sleep 1.5
check_wait "4 совпадения, активное первое" "marks=4 active=0"; shot 01-found
xdotool key Return; sleep 1
check_wait "Enter — следующее" "active=1"
xdotool key shift+Return; sleep 1
check_wait "Shift+Enter — предыдущее" "active=0"
xdotool key shift+Return; sleep 1
check_wait "предыдущее с первого — на последнее (по кругу)" "active=3"
# регистр: «Aa» включает учёт регистра; клик по флажку справа от поля
xdotool key ctrl+a; xdotool type --delay 60 "Alpha"; sleep 1.5
check_wait "тот же запрос в другом регистре: снова 4 (регистр не учитывается)" "marks=4"
xdotool key Escape; sleep 1.5
check_wait "Esc убирает метки" "marks=0"
gui_finish
