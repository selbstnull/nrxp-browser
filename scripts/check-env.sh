#!/usr/bin/env bash
# Проверка окружения для сборки Servo (Linux, Debian/Ubuntu). Ничего не устанавливает — только печатает отчёт
# и готовую команду. Всегда exit 0: это информация, а не ошибка.
set -u
missing_bins=()
for b in clang cmake python3 pkg-config make gcc g++ git curl; do
  command -v "$b" >/dev/null 2>&1 || missing_bins+=("$b")
done

missing_pkgs=()
need_pkgs=(
  clang cmake libclang-dev llvm-dev
  libfontconfig1-dev libfreetype-dev libunwind-dev libudev-dev
  libx11-dev libxcb1-dev libxkbcommon-dev libwayland-dev libegl1-mesa-dev libgles2-mesa-dev
  libssl-dev libdbus-1-dev libvulkan-dev mesa-vulkan-drivers libgl1-mesa-dri
)
if command -v dpkg >/dev/null 2>&1; then
  for p in "${need_pkgs[@]}"; do
    dpkg -s "$p" >/dev/null 2>&1 || missing_pkgs+=("$p")
  done
fi

echo "== rust =="; rustc --version 2>&1; cargo --version 2>&1
echo "== cpu/ram/disk =="; nproc; free -g | awk 'NR==2{print $2" GiB RAM"}'; df -h . | tail -1
echo "== display =="; echo "DISPLAY=${DISPLAY:-} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-}"
echo "== optional: GStreamer (для <video>/<audio>, фича media-gstreamer) =="
if pkg-config --exists gstreamer-1.0 gstreamer-app-1.0 gstreamer-plugins-bad-1.0 gstreamer-gl-1.0; then echo "gstreamer dev: ok (сборка: cargo build -p nrb --features media)"
else echo "gstreamer dev: нет — sudo apt install libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev libgstreamer-plugins-bad1.0-dev gstreamer1.0-plugins-good gstreamer1.0-libav"; fi
echo "== не найдены бинарники =="; printf '%s\n' "${missing_bins[@]:-—}"
echo "== не установлены пакеты =="; printf '%s\n' "${missing_pkgs[@]:-—}"
if [ "${#missing_pkgs[@]}" -gt 0 ]; then
  echo
  echo "Установка:  sudo apt-get install -y ${missing_pkgs[*]}"
fi
exit 0
