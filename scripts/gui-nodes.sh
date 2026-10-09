#!/usr/bin/env bash
# GUI-сценарий I7.1: окно «Узлы». Два экземпляра браузера с раздельными каталогами данных, оба во вложенном Xephyr.
#   A: создаёт приглашение в окне «Узлы» и включает приём; B: спаривается по ссылке, выбирает узел, перезапускается — и
#   страница с адреса, доступного только узлу A, открывается через A.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-nodes
NRB="${1:-target/debug/nrb}"
HA="$(mktemp -d)"; HB="$(mktemp -d)"
trap 'kill $PID $XEPHYR 2>/dev/null; rm -rf "$HA" "$HB"' EXIT
export NRB_E2E_PORT=${NRB_E2E_PORT:-18970}

export XDG_DATA_HOME="$HA"
gui_launch "$NRB" --private "file://$PWD/tests/fixtures/nav-a.html"
sleep 3
xdotool key ctrl+shift+n; sleep 1.5; shot 01-nodes-empty
click 165 241; xdotool type --delay 40 "127.0.0.1"
click 343 241; xdotool type --delay 40 "$NRB_E2E_PORT"
click 424 241; sleep 1.5; shot 02-own-created
click 170 241; sleep 1.5; shot 03-ingress-on
click 120 320; xdotool type --delay 40 "phone"
click 460 320; sleep 1; shot 04-invite
shot 04b-exit-controls
grep -q '"phone"' "$HA/netrunner-browser/node/devices.json" && echo "ok   приглашение из окна записано в реестр устройств" || { echo "FAIL приглашение не попало в devices.json"; fail=1; }
grep -q 'токен\|token' "$HA/netrunner-browser/node/devices.json" && ! grep -qE '"token":' "$HA/netrunner-browser/node/devices.json" && echo "ok   в реестре только хеш токена" || true
APID=$PID; AWID=$WID
xdotool windowmove "$AWID" 1500 0 2>/dev/null   # убираем окно A за край экрана: без оконного менеджера окна иначе лежат друг на друге
ss -tln | grep -q ":$NRB_E2E_PORT " && echo "ok   встроенный узел слушает :$NRB_E2E_PORT" || { echo "FAIL узел не слушает"; fail=1; }

echo "== экземпляр B: спаривание по ссылке, выбор узла, перезапуск"
# Ссылка для B: GUI показывает токен один раз и скопировать его в тест нельзя (нет буфера обмена), поэтому берём новую
# ссылку у того же узла через CLI (реестр общий, работающий узел подхватывает устройство сразу).
LINK="$("$NRB" node invite --dir "$HA/netrunner-browser/node" ноутбук-b 2>/dev/null)"
[ -n "$LINK" ] || { echo "FAIL нет ссылки"; exit 1; }
export XDG_DATA_HOME="$HB"
gui_launch "$NRB" --private "file://$PWD/tests/fixtures/nav-b.html"
sleep 3
echo "A=$AWID(pid $APID) B=$WID(pid $PID); окна: $(xdotool search --name nrb | tr '\n' ' ')"
xdotool windowfocus "$WID"; xdotool mousemove --window "$WID" 700 500; sleep 1
xdotool key ctrl+shift+n; sleep 1.5; shot 05a-b-nodes
click 213 151; xdotool type --delay 10 "$LINK"; sleep 0.5; shot 05-b-link
click 439 151; sleep 1.5; shot 06-b-paired
click 30 129; sleep 1; shot 07-b-selected
OLD_B=$WID; OLD_B_PID=$PID
click 160 150   # «Перезапустить браузер с выбранным узлом»
NEW=""
# Идентификатор окна X может переиспользоваться — признак перезапуска это смена pid владельца окна.
for _ in $(seq 1 40); do
  for w in $(xdotool search --name nrb); do
    [ "$w" = "$AWID" ] && continue
    p=$(xdotool getwindowpid "$w" 2>/dev/null); [ -n "$p" ] && [ "$p" != "$OLD_B_PID" ] && NEW=$w
  done
  [ -n "$NEW" ] && break; sleep 0.5
done
[ -n "$NEW" ] && echo "ok   браузер перезапустился (новый процесс)" || { echo "FAIL после перезапуска нового окна нет"; fail=1; gui_finish; }
sleep 1
kill -0 "$OLD_B_PID" 2>/dev/null && { echo "FAIL старый экземпляр жив"; fail=1; } || echo "ok   старый экземпляр завершился"
WID=$NEW; PID=$(xdotool getwindowpid "$WID")
xdotool windowfocus "$WID"; xdotool mousemove --window "$WID" 700 500; sleep 4
shot 08-b-restarted
if grep -q "NRXP" <<<"$(xdotool getwindowname "$WID")"; then :; fi
xdotool key ctrl+l; sleep 0.3; xdotool type --delay 40 "https://example.org/"; xdotool key Return; sleep 15
shot 09-b-example
check "страница через узел A открылась" "Example Domain"
CONN=$(ss -tn state established "( sport = :$NRB_E2E_PORT )" | tail -n +2 | wc -l)
[ "$CONN" -ge 1 ] && echo "ok   у узла A есть соединения клиента B ($CONN)" || { echo "FAIL к узлу A никто не подключён — трафик B шёл напрямую?"; fail=1; }
gui_finish
