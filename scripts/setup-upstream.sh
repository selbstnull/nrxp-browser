#!/usr/bin/env bash
# Готовит `.nrb-dev/netrunner-proxy` — отдельный git-worktree соседнего репозитория netrunner-proxy на локальной
# ветке `nrb/upstream-fixes` с нашими патчами из patches/proxy/*.patch. На него смотрит крейт nrb-transport.
#
# Зачем worktree: в рабочем дереве netrunner-proxy есть чужие незакоммиченные/непушенные изменения — их нельзя трогать.
# Ничего не пушится. Когда патчи приняты в netrunner-proxy, в crates/nrb-transport/Cargo.toml путь меняется обратно на
# `../../../netrunner-proxy/core`, а worktree удаляется (`git -C ../netrunner-proxy worktree remove .nrb-dev/netrunner-proxy`).
set -eu
cd "$(dirname "$0")/.."
SRC="${NRB_PROXY_SRC:-../netrunner-proxy}"
WT=".nrb-dev/netrunner-proxy"
BRANCH="nrb/connect-reliable"

[ -d "$SRC/.git" ] || [ -f "$SRC/.git" ] || { echo "нет репозитория $SRC (задайте NRB_PROXY_SRC)"; exit 1; }
if [ -d "$WT" ]; then
  echo "worktree уже есть: $WT ($(git -C "$WT" log --oneline -1))"
else
  mkdir -p .nrb-dev
  if git -C "$SRC" rev-parse --verify -q "$BRANCH" >/dev/null; then
    git -C "$SRC" worktree add "$PWD/$WT" "$BRANCH"
  else
    git -C "$SRC" worktree add -b "$BRANCH" "$PWD/$WT" HEAD
    # Ядро с 2026-10-10 уже содержит наш коммит (netrunner-proxy main, a43a077; ядро идёт дальше, поэтому «обратное применение»
    # патча не проверяем — оно ломается от соседних правок): признак — файл reverse_egress.rs. Патчи нужны только старому ядру.
    if [ -f "$WT/core/src/net/connection/reverse_egress.rs" ]; then
      echo "ядро уже содержит reverse egress и надёжный Connect — патчи patches/proxy/ не нужны"
    else
      for patch in patches/proxy/*.patch; do
        git -C "$WT" -c user.name="${GIT_AUTHOR_NAME:-nrb}" -c user.email="${GIT_AUTHOR_EMAIL:-nrb@localhost}" am "$PWD/$patch"
      done
    fi
  fi
  echo "готово: $WT"
fi

# --- Servo: форк nrxp-servo (обязателен с N2: браузер использует `ServoBuilder::network_connector`, которого нет в crates.io).
# Результат — `.cargo/config.toml` с [patch.crates-io] (генерируется, в git не хранится). Подмена ВСЕХ крейтов servo* обязательна:
# частичная подмена даёт два набора несовместимых типов (path- и registry-версии одного крейта).
# NRB_NO_SERVO=1 — только ядро (проверки крейтов без движка в CI: Servo не нужен, цель — узел, туннель, профиль и т. п.).
if [ "${NRB_NO_SERVO:-0}" = 1 ]; then
  echo "NRB_NO_SERVO=1: Servo не подключаем (без .cargo/config.toml); собирать nrb с фичей servo в таком режиме нельзя"
  exit 0
fi
NRXP_SERVO_DIR="${NRXP_SERVO_DIR:-$PWD/../nrxp-servo}"
if [ ! -f "$NRXP_SERVO_DIR/components/servo/Cargo.toml" ]; then
  echo "нет форка Servo: $NRXP_SERVO_DIR (задайте NRXP_SERVO_DIR). Нужна ветка nrxp/0.7 репозитория nrxp-servo: см. docs/NRXP-SERVO.md"
  exit 1
fi
mkdir -p .cargo
# Windows: консоль Python по умолчанию в cp1252, а скрипты печатают русский текст.
export PYTHONUTF8=1 PYTHONIOENCODING=utf-8
# Windows: интерпретатор называется `python` (python3.exe может не быть).
PY="$(command -v python3 || command -v python || true)"
[ -n "$PY" ] || { echo "нужен python3 (или python)"; exit 1; }
"$PY" scripts/gen-servo-patch.py "$NRXP_SERVO_DIR" Cargo.lock > .cargo/config.toml
echo "готово: Servo из форка $NRXP_SERVO_DIR (ветка $(git -C "$NRXP_SERVO_DIR" branch --show-current)); .cargo/config.toml"
