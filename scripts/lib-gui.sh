# Общий код GUI-сценариев (source, не запускать). Всё выполняется во вложенном X-сервере (Xephyr): xdotool шлёт события
# только в него, поэтому ввод не может попасть в окна рабочего стола (в первом прогоне без этого ввод ушёл в поле чата).
#
# Использование в сценарии:
#   . "$(dirname "$0")/lib-gui.sh"; gui_init out/имя; gui_launch "$NRB" [аргументы nrb…]; …; gui_finish
# Функции: check «описание» «подстрока заголовка» · title · click X Y (координаты окна) · typestr «текст».

gui_init() {
  cd "$(dirname "${BASH_SOURCE[1]}")/.."
  HOST_DISPLAY="${DISPLAY:-:1}"
  NESTED=":77"
  DISPLAY="$HOST_DISPLAY" Xephyr "$NESTED" -screen 1400x900 -ac -no-host-grab >/dev/null 2>&1 &
  XEPHYR=$!
  export DISPLAY="$NESTED"
  for _ in $(seq 1 20); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.25; done
  [ "$DISPLAY" != "$HOST_DISPLAY" ] || { echo "ПРЕРВАНО: сценарий обязан работать во вложенном X-сервере"; exit 2; }
  OUT="$1"; mkdir -p "$OUT"
  fail=0
  PID=""
  trap 'kill $PID $XEPHYR 2>/dev/null' EXIT
}

# gui_launch <nrb> [аргументы…] — запускает окно и ждёт его появления; после — $WID.
gui_launch() {
  local nrb="$1"; shift
  unset WID   # иначе при втором запуске в одном сценарии цикл ниже сразу принял бы окно предыдущего экземпляра
  "$nrb" "$@" > "$OUT/nrb.log" 2>&1 &
  PID=$!
  for _ in $(seq 1 40); do WID=$(xdotool search --all --pid "$PID" --name "nrb" 2>/dev/null | tail -1); [ -n "${WID:-}" ] && break; sleep 0.5; done
  [ -n "${WID:-}" ] || { echo "FAIL окно не появилось"; exit 1; }
  # Без оконного менеджера фокус следует за указателем — ставим его в окно.
  xdotool windowfocus "$WID"
  xdotool mousemove --window "$WID" 640 400
}

title() { xdotool getwindowname "$WID" 2>/dev/null; }

check() { # описание, подстрока заголовка
  local t; t=$(title)
  if [[ "$t" == *"$2"* ]]; then echo "ok   $1 ($t)"; else echo "FAIL $1: ожидалось «$2», заголовок «$t»"; fail=1; fi
}

click() { xdotool mousemove --window "$WID" "$1" "$2"; sleep 0.25; xdotool click 1; }
shot() { import -window "$WID" "$OUT/$1.png"; }

gui_finish() {
  echo; [ "$fail" -eq 0 ] && echo "GUI-сценарий: всё прошло" || echo "GUI-сценарий: есть падения"
  exit "$fail"
}
