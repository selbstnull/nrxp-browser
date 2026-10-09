#!/usr/bin/env bash
# Запуск тяжёлой команды (сборка Servo, тесты) с потолками ресурсов, чтобы машина не ложилась:
#   scripts/lowload.sh cargo build -p nrb --features servo
# Потолки через cgroup (systemd-run --user --scope): память (при превышении мягкого порога процесс притормаживается,
# при жёстком — убивается только эта команда, а не рабочий стол), CPU, приоритеты CPU/диска. Настройка переменными:
#   LOWLOAD_MEM_HIGH=14G  LOWLOAD_MEM_MAX=20G  LOWLOAD_CPU=600  (в процентах одного ядра)  LOWLOAD_JOBS=4
set -eu
JOBS="${LOWLOAD_JOBS:-4}"
export CARGO_BUILD_JOBS="$JOBS"
export RUST_TEST_THREADS="${RUST_TEST_THREADS:-2}"
if command -v systemd-run >/dev/null && systemd-run --user --scope -p MemoryMax=1G true >/dev/null 2>&1; then
  exec systemd-run --user --scope --quiet \
    -p MemoryHigh="${LOWLOAD_MEM_HIGH:-14G}" -p MemoryMax="${LOWLOAD_MEM_MAX:-20G}" -p MemorySwapMax=0 \
    -p CPUQuota="${LOWLOAD_CPU:-600}%" -p IOWeight=20 \
    nice -n 19 ionice -c3 "$@"
fi
echo "lowload: cgroup-лимиты недоступны, работаю только с nice/ionice и CARGO_BUILD_JOBS=$JOBS" >&2
exec nice -n 19 ionice -c3 "$@"
