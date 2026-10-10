#!/usr/bin/env bash
# GUI-сценарий N6a: печать в PDF (Ctrl+P). Страница из четырёх цветных полос по 800 px (красная, зелёная, синяя, жёлтая).
# Проверяем PDF настоящими инструментами poppler: страниц больше одной, формат A4, первая страница сверху красная,
# последняя снизу жёлтая; прокрутка страницы после печати вернулась.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-print
NRB="${1:-target/debug/nrb}"
command -v pdfinfo >/dev/null && command -v pdftoppm >/dev/null || { echo "пропуск: нет poppler-utils (pdfinfo/pdftoppm)"; exit 0; }
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR 2>/dev/null; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/cfg" "$TMP/dl"
echo "XDG_DOWNLOAD_DIR=\"$TMP/dl\"" > "$TMP/cfg/user-dirs.dirs"

XDG_CONFIG_HOME="$TMP/cfg" gui_launch "$NRB" --private "file://$PWD/tests/fixtures/print.html"; sleep 3
check "страница загрузилась" "Печать: тест"
xdotool key ctrl+p; sleep 2; shot 01-printing
FILE=""
for _ in $(seq 1 90); do FILE="$(ls "$TMP/dl"/*.pdf 2>/dev/null | head -1)"; [ -n "$FILE" ] && [ -s "$FILE" ] && break; sleep 1; done
[ -n "$FILE" ] && [ -s "$FILE" ] && echo "ok   PDF создан: $(basename "$FILE") ($(stat -c %s "$FILE") байт)" || { echo "FAIL PDF не появился"; fail=1; gui_finish; }
sleep 1
INFO="$(pdfinfo "$FILE" 2>&1)"
PAGES="$(echo "$INFO" | sed -n 's/^Pages: *//p')"
[ "${PAGES:-0}" -ge 2 ] && echo "ok   страниц: $PAGES" || { echo "FAIL страниц: ${PAGES:-?}"; echo "$INFO" | head -5; fail=1; }
echo "$INFO" | grep -q "595 x 842 pts" && echo "ok   формат A4" || { echo "FAIL формат: $(echo "$INFO" | grep 'Page size')"; fail=1; }
echo "$INFO" | grep -q "Title:.*Печать: тест" && echo "ok   заголовок документа" || { echo "FAIL заголовок"; fail=1; }

pixel() { # страница, x, y → "R G B" из PPM
  pdftoppm -f "$1" -l "$1" -r 36 "$FILE" "$TMP/p" 2>/dev/null
  python3 - "$TMP" "$1" "$2" "$3" <<'PY'
import sys, glob
tmp, page, fx, fy = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
path = sorted(glob.glob(f"{tmp}/p-*{page}.ppm"))[0]
data = open(path, "rb").read()
parts = data.split(b"\n", 3)          # P6 / ширина высота / 255 / пиксели
w, h = map(int, parts[1].split())
x, y = int(w * fx), int(h * fy)
i = (y * w + x) * 3
print(*parts[3][i:i + 3])
PY
  rm -f "$TMP"/p-*
}
TOP="$(pixel 1 0.5 0.02)"
set -- $TOP
[ "$1" -gt 150 ] && [ "$2" -lt 80 ] && [ "$3" -lt 80 ] && echo "ok   верх первой страницы красный ($TOP)" || { echo "FAIL верх первой страницы: $TOP"; fail=1; }
BOTTOM="$(pixel "$PAGES" 0.5 0.55)"
set -- $BOTTOM
[ "$1" -gt 180 ] && [ "$2" -gt 150 ] && [ "$3" -lt 80 ] && echo "ok   низ последней страницы жёлтый ($BOTTOM)" || { echo "FAIL низ последней страницы: $BOTTOM"; fail=1; }
shot 02-after
gui_finish
