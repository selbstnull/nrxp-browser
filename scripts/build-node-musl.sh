#!/usr/bin/env bash
# Статический бинарь `nrb` (musl) без движка — для заливки на любой Linux-сервер: `nrb deploy user@host --binary <этот файл>`.
# Требует musl-tools и rustup target x86_64-unknown-linux-musl (для aarch64 — свой target и кросс-компилятор).
set -eu
cd "$(dirname "$0")/.."
TARGET="${1:-x86_64-unknown-linux-musl}"
command -v musl-gcc >/dev/null || { echo "нет musl-gcc: sudo apt install musl-tools"; exit 1; }
rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"
CARGO_TARGET_DIR=target-release cargo build --release -p nrb --target "$TARGET"
OUT="target-release/$TARGET/release/nrb"
file "$OUT" | cut -c1-120
echo "готово: $OUT ($(du -h "$OUT" | cut -f1))"
