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
if [ -d "$WT" ]; then echo "worktree уже есть: $WT ($(git -C "$WT" log --oneline -1))"; exit 0; fi

mkdir -p .nrb-dev
if git -C "$SRC" rev-parse --verify -q "$BRANCH" >/dev/null; then
  git -C "$SRC" worktree add "$PWD/$WT" "$BRANCH"
else
  git -C "$SRC" worktree add -b "$BRANCH" "$PWD/$WT" HEAD
  for patch in patches/proxy/*.patch; do
    git -C "$WT" -c user.name="${GIT_AUTHOR_NAME:-nrb}" -c user.email="${GIT_AUTHOR_EMAIL:-nrb@localhost}" am "$PWD/$patch"
  done
fi
echo "готово: $WT"
