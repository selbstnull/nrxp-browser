#!/usr/bin/env bash
# Упаковка релиза (I14): tar.gz и .deb из release-сборки с движком. Использование: scripts/package.sh [версия]
# Бинарь: target-release/release/nrb (CARGO_TARGET_DIR=target-release cargo build --release -p nrb --features servo).
# Зависимости deb вычисляются по `ldd` (пакеты, владеющие найденными библиотеками), а не пишутся вручную.
set -eu
cd "$(dirname "$0")/.."
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' nrb/Cargo.toml | head -1)}"
BIN="${NRB_BIN:-target-release/release/nrb}"
[ -x "$BIN" ] || { echo "нет $BIN: соберите release с движком"; exit 1; }
"$BIN" version | grep -q "nrb" || { echo "бинарь не запускается"; exit 1; }
ARCH="$(uname -m)"; DEB_ARCH=$([ "$ARCH" = x86_64 ] && echo amd64 || echo "$ARCH")
OUT=dist; rm -rf "$OUT/stage"; mkdir -p "$OUT/stage"

# --- tar.gz
T="$OUT/stage/nrb-$VERSION-linux-$ARCH"
mkdir -p "$T"
install -m755 "$BIN" "$T/nrb"
install -m644 packaging/nrb.desktop packaging/nrb.svg README.md "$T/"
cat > "$T/install.sh" <<'INS'
#!/bin/sh
# Установка в ~/.local (без root): бинарь, ярлык и значок.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
install -Dm755 "$here/nrb" "$prefix/bin/nrb"
install -Dm644 "$here/nrb.desktop" "$prefix/share/applications/nrb.desktop"
install -Dm644 "$here/nrb.svg" "$prefix/share/icons/hicolor/scalable/apps/nrb.svg"
echo "nrb установлен в $prefix/bin/nrb"
INS
chmod +x "$T/install.sh"
tar -C "$OUT/stage" -czf "$OUT/nrb-$VERSION-linux-$ARCH.tar.gz" "nrb-$VERSION-linux-$ARCH"

# --- deb
D="$OUT/stage/deb"
mkdir -p "$D/DEBIAN" "$D/usr/bin" "$D/usr/share/applications" "$D/usr/share/icons/hicolor/scalable/apps"
install -m755 "$BIN" "$D/usr/bin/nrb"
install -m644 packaging/nrb.desktop "$D/usr/share/applications/nrb.desktop"
install -m644 packaging/nrb.svg "$D/usr/share/icons/hicolor/scalable/apps/nrb.svg"
DEPS="$(ldd "$BIN" | awk '/=> \//{print $3}' | xargs -r dpkg -S 2>/dev/null | cut -d: -f1 | sort -u | paste -sd, - | sed 's/,/, /g')"
SIZE="$(du -sk "$D/usr" | cut -f1)"
cat > "$D/DEBIAN/control" <<CTL
Package: nrb
Version: $VERSION
Section: web
Priority: optional
Architecture: $DEB_ARCH
Installed-Size: $SIZE
Depends: $DEPS
Maintainer: nrb <nrb@localhost>
Description: Браузер на Rust поверх NRXP (движок Servo)
 Сырой проект: не проверен человеком, не для прода.
CTL
dpkg-deb --build --root-owner-group "$D" "$OUT/nrb_${VERSION}_$DEB_ARCH.deb" > /dev/null
rm -rf "$OUT/stage"
(cd "$OUT" && sha256sum nrb-"$VERSION"-linux-"$ARCH".tar.gz nrb_"${VERSION}"_"$DEB_ARCH".deb > SHA256SUMS)
ls -la "$OUT"
