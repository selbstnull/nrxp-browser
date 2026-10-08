#!/usr/bin/env bash
# GUI-сценарий I11: окно браузера в герметичном режиме. Свой узел на loopback; `nrb --hermetic --node …` открывает
# страницу через релей и туннель; по /proc проверяем: потоки туннеля — в сети хоста, остальные (движок) — в пустом
# пространстве имён, где нет ничего кроме lo.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-hermetic
NRB="${1:-target/debug/nrb}"
DIR="$(mktemp -d)"; PORT="${NRB_E2E_PORT:-18965}"
trap 'kill $PID $XEPHYR $NODE_PID 2>/dev/null; rm -rf "$DIR"' EXIT
"$NRB" node init --dir "$DIR/node" --public-host 127.0.0.1 --port "$PORT" > /dev/null || exit 1
"$NRB" node run --dir "$DIR/node" > "$DIR/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && break; sleep 0.25; done
LINK="$("$NRB" node invite --dir "$DIR/node" окно 2>/dev/null)"

gui_launch "$NRB" --hermetic --node "$LINK" https://example.org/
sleep 12
check "страница открылась через релей и туннель" "Example Domain"; shot 01-hermetic

HOST_NS=$(readlink /proc/$$/ns/net)
NSES=$(for t in /proc/$PID/task/*; do readlink "$t/ns/net"; done | sort | uniq -c)
echo "потоки по сетевым пространствам имён:"; echo "$NSES" | sed 's/^/   /'
[ "$(echo "$NSES" | wc -l)" -eq 2 ] && echo "ok   у процесса ровно два сетевых пространства имён (хост и пустое)" || { echo "FAIL ожидалось 2 пространства имён"; fail=1; }
BAD=$(for t in /proc/$PID/task/*; do n=$(cat "$t/comm"); ns=$(readlink "$t/ns/net"); case "$n" in nrb-tunnel*|nrb-own-node*) [ "$ns" = "$HOST_NS" ] || echo "$n в $ns";; esac; done)
[ -z "$BAD" ] && echo "ok   потоки туннеля и менеджера узлов — в сети хоста" || { echo "FAIL потоки вне сети хоста: $BAD"; fail=1; }
ENGINE_HOST=$(for t in /proc/$PID/task/*; do n=$(cat "$t/comm"); ns=$(readlink "$t/ns/net"); case "$n" in nrb-tunnel*|nrb-own-node*|nrb-relay) ;; *) [ "$ns" = "$HOST_NS" ] && echo "$n";; esac; done | sort | uniq -c)
echo "потоки не из туннеля, оставшиеся в сети хоста (должно быть пусто, кроме основного потока до isolate):"; echo "${ENGINE_HOST:-   (нет)}" | sed 's/^/   /'
[ -z "$ENGINE_HOST" ] && echo "ok   потоков движка в сети хоста нет" || { echo "FAIL часть потоков осталась в сети хоста"; fail=1; }
grep -q "герметично" <(xdotool getwindowname "$WID") || true
gui_finish
