#!/usr/bin/env bash
# GUI-сценарий N5b: поиск по странице (Ctrl+F) в движке. Проверяем два свойства:
#  * подсветка рисуется движком: на скриншоте есть жёлтые (остальные) и оранжевые (активное) пиксели, Enter/Shift+Enter двигают
#    оранжевое, Esc убирает всё;
#  * DOM не трогается: страница считает записи MutationObserver и диапазоны выделения — в заголовке всё время то же, что было до поиска (muts=N sel=0).
# Видимых совпадений «alpha» без учёта регистра: Alpha, alpha, ALPHA, alpha (в конце) = 4; скрытый абзац и скрипт не считаются.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-find
NRB="${1:-target/debug/nrb}"
gui_launch "$NRB" --private "file://$PWD/tests/fixtures/find.html"; sleep 3
check_wait "страница загрузилась" "find: muts="
sleep 1
BASE=$(title | sed -E 's/^find: (muts=[0-9]+ sel=[0-9]+).*/\1/')   # записи самой страницы при разборе не в счёт: сравниваем с тем, что было до поиска
echo "исходное состояние: $BASE"
shot 00-before

# colors <png> → "жёлтых оранжевых cx cy" (центр оранжевого; -1 -1, если его нет). Цвета — полупрозрачные поверх белого.
colors() {
  python3 - "$1" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB"); w, h = im.size
px = im.load(); yellow = orange = sx = sy = 0
for y in range(56, h):            # ниже тулбара
    for x in range(w):
        r, g, b = px[x, y]
        if r > 240 and 225 <= g <= 250 and 110 <= b <= 160: yellow += 1
        elif r > 240 and 150 <= g <= 190 and b < 70: orange += 1; sx += x; sy += y
print(yellow, orange, (sx // orange) if orange else -1, (sy // orange) if orange else -1)
PY
}
expect_colors() { # описание, файл, «есть»/«нет»
  local r; r=$(colors "$2"); set -- "$1" "$3" $r
  if [ "$2" = есть ] && [ "$3" -gt 100 ] && [ "$4" -gt 100 ]; then echo "ok   $1 (жёлтых $3, оранжевых $4)"
  elif [ "$2" = нет ] && [ "$3" -lt 100 ] && [ "$4" -eq 0 ]; then echo "ok   $1 (жёлтых $3, оранжевых $4)"
  else echo "FAIL $1: жёлтых $3, оранжевых $4"; fail=1; fi
}

xdotool key ctrl+f; sleep 0.7
xdotool type --delay 60 "alpha"; sleep 2
shot 01-found; expect_colors "поиск подсвечивает совпадения" out/gui-find/01-found.png есть
read -r _ _ cx0 cy0 <<<"$(colors out/gui-find/01-found.png)"
xdotool key Return; sleep 1.2
shot 02-next; read -r _ _ cx1 cy1 <<<"$(colors out/gui-find/02-next.png)"
if [ "$cx1" != "$cx0" ] || [ "$cy1" != "$cy0" ]; then echo "ok   Enter сдвинул активное ($cx0,$cy0 → $cx1,$cy1)"; else echo "FAIL Enter не сдвинул активное ($cx0,$cy0)"; fail=1; fi
xdotool key shift+Return; sleep 1.2
xdotool key shift+Return; sleep 1.2
shot 03-wrap; read -r _ _ cx3 cy3 <<<"$(colors out/gui-find/03-wrap.png)"
if [ "$cy3" -gt "$cy0" ]; then echo "ok   Shift+Enter с первого — по кругу на последнее (y $cy0 → $cy3)"; else echo "FAIL по кругу: y $cy0 → $cy3"; fail=1; fi
check "DOM не менялся и выделения нет" "find: $BASE"
# Другой регистр — те же совпадения.
xdotool key ctrl+a; xdotool type --delay 60 "ALPHA"; sleep 2
shot 04-case; expect_colors "поиск без учёта регистра" out/gui-find/04-case.png есть
check "DOM не менялся и после смены запроса" "find: $BASE"
xdotool key Escape; sleep 1.5
shot 05-closed; expect_colors "Esc убирает подсветку" out/gui-find/05-closed.png нет
check "DOM не менялся и после закрытия" "find: $BASE"
gui_finish
