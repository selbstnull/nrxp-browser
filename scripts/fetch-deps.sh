#!/usr/bin/env bash
# Скачивание зависимостей с повторами: crates.io CDN периодически отдаёт 500/таймаут на больших крейтах Servo.
set -u
cd "$(dirname "$0")/.."
export CARGO_NET_RETRY=10 CARGO_HTTP_TIMEOUT=120
for i in $(seq 1 "${ATTEMPTS:-20}"); do
  if cargo fetch >/tmp/nrb-fetch.log 2>&1; then echo "FETCH_OK попытка $i"; exit 0; fi
  echo "попытка $i не удалась: $(grep -m1 'failed to get\|error' /tmp/nrb-fetch.log)"; sleep 5
done
echo "FETCH_GIVEUP"; exit 1
