#!/usr/bin/env bash
# Проверка I11: герметичный режим. Поднимает свой узел и обычный TCP-слушатель на loopback хоста, затем запускает
# `nrb hermetic-selftest` — он входит в герметичный режим и проверяет, что прямые соединения невозможны, а через релей
# и туннель страница открывается. Использование: scripts/hermetic-e2e.sh [путь/к/nrb]
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"
DIR="$(mktemp -d)"
PORT="${NRB_E2E_PORT:-18960}"; HOSTPORT="${NRB_E2E_HOSTPORT:-18961}"
cleanup() { for p in ${NODE_PID:-} ${WEB_PID:-}; do kill "$p" 2>/dev/null; done; rm -rf "$DIR"; }
trap cleanup EXIT

python3 -m http.server "$HOSTPORT" --bind 127.0.0.1 --directory "$DIR" > /dev/null 2>&1 &
WEB_PID=$!
"$NRB" node init --dir "$DIR/node" --public-host 127.0.0.1 --port "$PORT" > /dev/null || exit 1
"$NRB" node run --dir "$DIR/node" > "$DIR/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && ss -tln | grep -q ":$HOSTPORT " && break; sleep 0.25; done
LINK="$("$NRB" node invite --dir "$DIR/node" герметик 2>/dev/null)"
"$NRB" hermetic-selftest --node "$LINK" --host-port "$HOSTPORT"
