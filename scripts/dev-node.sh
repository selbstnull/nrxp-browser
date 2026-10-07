#!/usr/bin/env bash
# Локальный узел NRXP для разработки: `netrunner-server` из соседнего репозитория, анонимная схема, loopback.
# Использование: scripts/dev-node.sh [порт]   (по умолчанию 18443)   Затем: nrb fetch <url> --node-addr 127.0.0.1:<порт>
# ВНИМАНИЕ: анонимный узел без аутентификации — только на 127.0.0.1, никогда не выставлять наружу.
set -eu
cd "$(dirname "$0")/.."
PORT="${1:-18443}"
BIN="${NRB_SERVER_BIN:-}"
if [ -z "$BIN" ]; then
  for candidate in ../netrunner-proxy/target/release/netrunner-server ../netrunner-proxy/target/debug/netrunner-server; do
    [ -x "$candidate" ] && BIN="$candidate" && break
  done
fi
[ -n "$BIN" ] || { echo "нет бинаря netrunner-server: соберите (cargo build -p netrunner-server --release в netrunner-proxy) или задайте NRB_SERVER_BIN"; exit 1; }
echo "узел: $BIN → 127.0.0.1:$PORT"
exec "$BIN" --host 127.0.0.1 --port "$PORT"
