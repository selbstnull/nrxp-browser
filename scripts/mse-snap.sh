#!/usr/bin/env bash
# Проверка MSE (N7b): раздельные fMP4 видео и звука добавляются в два SourceBuffer, ролик играет; опционально перемотка.
# Нужны сборка с `--features media`, ffmpeg и системный GStreamer. Использование: scripts/mse-snap.sh [путь/к/nrb] [seek-секунды]
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"; SEEK="${2:-}"
command -v ffmpeg >/dev/null || { echo "пропуск: нет ffmpeg"; exit 0; }
TMP="$(mktemp -d)"; PORT="${NRB_E2E_PORT:-18973}"
trap 'kill "${WEB_PID:-}" 2>/dev/null; rm -rf "$TMP"' EXIT
FRAG="-movflags frag_keyframe+empty_moov+default_base_moof"
ffmpeg -v error -y -f lavfi -i testsrc=size=640x360:rate=30 -t 8 -c:v libx264 -g 30 -pix_fmt yuv420p -profile:v high -level 3.0 -an $FRAG "$TMP/video.mp4"
ffmpeg -v error -y -f lavfi -i sine=frequency=440 -t 8 -c:a aac -vn -frag_duration 1000000 -movflags empty_moov+default_base_moof "$TMP/audio.mp4"
cp tests/fixtures/mse.html "$TMP/"
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$TMP" >/dev/null 2>&1 &
WEB_PID=$!
sleep 1
URL="http://127.0.0.1:$PORT/mse.html"; [ -n "$SEEK" ] && URL="$URL?seek=$SEEK&chunk=1"
OUT="$("$NRB" snap "$URL" -o "$TMP/shot.png" --settle 6s --json 2>/dev/null)"
echo "$OUT"
case "$OUT" in
  *'"title":"mse: playing'*|*'"title":"mse: seeked'*|*'"title":"mse: ended'*) echo "всё прошло"; cp "$TMP/shot.png" "${MSE_SHOT:-/dev/null}" 2>/dev/null; exit 0 ;;
  *) echo "ПРОВАЛ: ролик не играет"; exit 1 ;;
esac
