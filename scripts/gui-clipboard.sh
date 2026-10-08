#!/usr/bin/env bash
# GUI-сценарий: буфер обмена страницы. Ввести текст в первое поле, Ctrl+A, Ctrl+C, вставить Ctrl+V во второе; потом Ctrl+X.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-clipboard
NRB="${1:-target/debug/nrb}"
gui_launch "$NRB" --private "file://$PWD/tests/fixtures/clipboard.html"; sleep 3
click 160 100; xdotool type --delay 40 "hello-world"; sleep 0.5
check_wait "текст введён" "a=hello-world"
xdotool key ctrl+a; xdotool key ctrl+c; sleep 0.5
# Системный буфер обмена (не только внутри страницы): читаем его другим клиентом X (tkinter), если он есть.
if python3 -c "import tkinter" 2>/dev/null; then
  SYS=$(python3 -c "import tkinter as t; r=t.Tk(); r.withdraw(); print(r.clipboard_get())" 2>&1 | tail -1)
  [ "$SYS" = "hello-world" ] && echo "ok   текст попал в системный буфер обмена" || { echo "FAIL системный буфер: «$SYS»"; fail=1; }
else
  echo "пропуск: нет tkinter для чтения системного буфера"
fi
click 160 170; xdotool key ctrl+v; sleep 1
check_wait "Ctrl+C / Ctrl+V между полями страницы" "b=hello-world"; shot 01-pasted
click 160 100; xdotool key ctrl+a; xdotool key ctrl+x; sleep 0.5
check_wait "Ctrl+X вырезал текст" "a=;b=hello-world"
gui_finish
