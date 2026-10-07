#!/usr/bin/env bash
# GUI-смоук I2: запускает окно nrb на $DISPLAY и проходит сценарий через xdotool, проверяя заголовок окна.
# Требует X11 (xdotool, imagemagick `import`). Сеть нужна (используются реальные сайты, кроме локальной фикстуры).
# Использование: scripts/gui-smoke.sh [путь/к/nrb]   Скриншоты — в out/gui-smoke/.
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"
# Всё выполняется во вложенном X-сервере (Xephyr): xdotool шлёт события только в него, поэтому ввод
# не может попасть в окна рабочего стола (в первом прогоне без этого ввод ушёл в поле чата).
HOST_DISPLAY="${DISPLAY:-:1}"
NESTED=":77"
DISPLAY="$HOST_DISPLAY" Xephyr "$NESTED" -screen 1400x900 -ac -no-host-grab >/dev/null 2>&1 &
XEPHYR=$!
export DISPLAY="$NESTED"
for _ in $(seq 1 20); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.25; done
[ "$DISPLAY" != "$HOST_DISPLAY" ] || { echo "ПРЕРВАНО: смоук обязан работать во вложенном X-сервере"; exit 2; }
OUT=out/gui-smoke; mkdir -p "$OUT"
FIXTURE="file://$PWD/tests/fixtures/input.html"
fail=0

check() { # описание, подстрока заголовка
  local title; title=$(xdotool getwindowname "$WID" 2>/dev/null)
  if [[ "$title" == *"$2"* ]]; then echo "ok   $1 ($title)"; else echo "FAIL $1: ожидалось «$2», заголовок «$title»"; fail=1; fi
}

# NRB_ARGS — доп. аргументы nrb, например "--node-addr 127.0.0.1:18443" (весь трафик движка через туннель).
# shellcheck disable=SC2086
"$NRB" ${NRB_ARGS:-} "https://example.org" > "$OUT/nrb.log" 2>&1 &
PID=$!
trap 'kill $PID $XEPHYR 2>/dev/null' EXIT
for _ in $(seq 1 40); do WID=$(xdotool search --pid "$PID" --name "nrb" 2>/dev/null | tail -1); [ -n "${WID:-}" ] && break; sleep 0.5; done
[ -n "${WID:-}" ] || { echo "FAIL окно не появилось"; exit 1; }
# Без оконного менеджера фокус следует за указателем — ставим его в окно.
xdotool windowfocus "$WID"   # безопасно: DISPLAY указывает на вложенный Xephyr
xdotool mousemove --window "$WID" 640 400
sleep 4
check "стартовая страница" "Example Domain"
import -window "$WID" "$OUT/01-start.png"

xdotool key ctrl+l; sleep 0.3; xdotool type --delay 40 "news.ycombinator.com"; xdotool key Return; sleep 6
check "омнибокс: ввод домена" "Hacker News"
import -window "$WID" "$OUT/02-hn.png"

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
import -window "$WID" "$OUT/03-search.png"
xdotool key ctrl+Tab; sleep 1.5
check "переключение вкладок (Ctrl+Tab)" "Page B"

xdotool key ctrl+t; sleep 0.8; xdotool type --delay 30 "$FIXTURE"; xdotool key Return; sleep 3
xdotool mousemove --window "$WID" 300 156; xdotool click 1; sleep 0.5
xdotool type --delay 80 "abc "; sleep 0.5; xdotool type --delay 250 "привет"; sleep 1
# xdotool перекартирует клавиатуру на лету и иногда дублирует первый символ кириллицы — проверяем хвост слова
check "ввод в поле страницы (ASCII + кириллица)" "ривет"
import -window "$WID" "$OUT/04-input.png"

xdotool key ctrl+w; sleep 1
echo; [ "$fail" -eq 0 ] && echo "GUI-смоук: всё прошло" || echo "GUI-смоук: есть падения"
exit "$fail"
