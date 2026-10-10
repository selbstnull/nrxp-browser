#!/usr/bin/env bash
# GUI-сценарий N7a: воспроизведение <video> (нужна сборка с `--features media` и GStreamer в системе; ffmpeg — для фикстур).
# Генерирует 3-секундные клипы (WebM VP8+Vorbis и MP4 H.264+AAC), страница tests/fixtures/media.html пишет прогресс в заголовок
# окна. Проверяем: метаданные загрузились, время идёт, ролик доиграл до конца.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-media
NRB="${1:-target/debug/nrb}"
command -v ffmpeg >/dev/null || { echo "пропуск: нет ffmpeg для генерации клипов"; exit 0; }
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT
cp tests/fixtures/media.html "$TMP/"
SRCV="-f lavfi -i testsrc=size=320x240:rate=25:duration=3 -f lavfi -i sine=frequency=440:duration=3"
# shellcheck disable=SC2086
ffmpeg -v error -y $SRCV -c:v libvpx -b:v 300k -c:a libvorbis "$TMP/clip.webm"
# shellcheck disable=SC2086
ffmpeg -v error -y $SRCV -c:v libx264 -pix_fmt yuv420p -c:a aac -movflags +faststart "$TMP/clip.mp4"
PORT=$((20000 + RANDOM % 20000))
(cd "$TMP" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SRV=$!
for _ in $(seq 1 30); do ss -tln | grep -q ":$PORT " && break; sleep 0.2; done

play() { # формат, файл
  gui_launch "$NRB" --private "http://127.0.0.1:$PORT/media.html?src=$2&muted=1"; sleep 3
  check_wait "$1: метаданные загрузились" "media: " 8
  local t="" ok=0
  for _ in $(seq 1 40); do t=$(title); [[ "$t" == *"ended"* ]] && ok=1 && break; [[ "$t" == *"error"* ]] && break; sleep 0.5; done
  shot "$1"
  if [ $ok = 1 ]; then echo "ok   $1: доиграл до конца ($t)"; else echo "FAIL $1: заголовок «$t»"; fail=1; fi
  kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1
}
play webm clip.webm
play mp4 clip.mp4
gui_finish
