#!/usr/bin/env bash
# Прогон корпуса совместимости (I3) для CI/ночных запусков. Собирает nrb с Servo, если бинаря нет.
# Использование: scripts/corpus.sh [аргументы nrb corpus…]   Код выхода ≠ 0, если процент ниже baseline.
set -eu
cd "$(dirname "$0")/.."
BIN="${NRB:-target/debug/nrb}"
[ -x "$BIN" ] || cargo build -p nrb --features servo
exec "$BIN" corpus "$@"
