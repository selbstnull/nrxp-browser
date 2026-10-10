#!/usr/bin/env bash
# GUI-сценарий «звук»: политика автовоспроизведения и звук вкладки (сборка --features media, нужны ffmpeg и PipeWire с pw-dump).
# 1) ролик со звуком без жеста не стартует (play() отклонён) и в PipeWire нет потока nrb;
# 2) после клика по кнопке «play» ролик играет и в PipeWire появляется выходной аудиопоток процесса nrb;
# 3) Ctrl+M (как клик по значку динамика) заглушает вкладку: поток замьючен, ролик при этом идёт (время растёт);
# 4) снова Ctrl+M — звук возвращается.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-audio
NRB="${1:-target/debug/nrb}"
command -v ffmpeg >/dev/null && command -v pw-dump >/dev/null || { echo "пропуск: нужны ffmpeg и pw-dump (PipeWire)"; exit 0; }
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $SRV 2>/dev/null; rm -rf "$TMP"' EXIT
cp tests/fixtures/media.html "$TMP/"
ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=25:duration=60 -f lavfi -i sine=frequency=440:duration=60 -c:v libvpx -b:v 100k -c:a libvorbis "$TMP/long.webm"
PORT=$((20000 + RANDOM % 20000))
(cd "$TMP" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
SRV=$!
for _ in $(seq 1 30); do ss -tln | grep -q ":$PORT " && break; sleep 0.2; done

# Число выходных аудиопотоков процесса nrb в PipeWire (state=running).
streams() {
  pw-dump 2>/dev/null | python3 -c "
import json,sys
pid=$PID
n=0
for o in json.load(sys.stdin):
    p=o.get('info',{}).get('props',{})
    if p.get('media.class','').startswith('Stream/Output/Audio') and p.get('application.process.id')==pid and o['info'].get('state')=='running': n+=1
print(n)"
}
wait_streams() { # ожидаемое «>0» или «0», секунд
  local want="$1" limit="${2:-8}" n=""
  for _ in $(seq 1 $((limit * 2))); do
    n=$(streams)
    if [ "$want" = ">0" ] && [ "${n:-0}" -gt 0 ]; then return 0; fi
    if [ "$want" = "0" ] && [ "${n:-0}" -eq 0 ]; then return 0; fi
    sleep 0.5
  done
  return 1
}

gui_launch "$NRB" --private "http://127.0.0.1:$PORT/media.html?src=long.webm&auto=0"; sleep 3
check_wait "страница загрузилась" "media: " 8
# Автозапуск без жеста: перезагружаем страницу с автозапуском (play() через 0,5 с после загрузки).
kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1
gui_launch "$NRB" --private "http://127.0.0.1:$PORT/media.html?src=long.webm"; sleep 4
check_wait "автозапуск со звуком без жеста заблокирован" "play-rejected NotAllowedError" 8
if wait_streams 0 4; then echo "ok   аудиопотока nrb в PipeWire нет"; else echo "FAIL поток есть при заблокированном автозапуске"; fail=1; fi

# Жест + кнопка.
click 120 $((56 + 335)); sleep 3
check_wait "после клика ролик играет" "media: playing" 14
if wait_streams ">0" 8; then echo "ok   в PipeWire появился аудиопоток nrb"; else echo "FAIL аудиопотока нет, хотя ролик играет"; fail=1; fi
shot 01-playing

# Ctrl+M — заглушить вкладку (то же, что клик по значку динамика): поток остаётся, но в PipeWire помечен mute, ролик идёт.
stream_mute() { # печатает true/false/none — mute у выходного потока nrb
  pw-dump 2>/dev/null | python3 -c "
import json,sys
pid=$PID
res='none'
for o in json.load(sys.stdin):
    p=o.get('info',{}).get('props',{})
    if p.get('media.class','').startswith('Stream/Output/Audio') and p.get('application.process.id')==pid:
        for prm in o['info'].get('params',{}).get('Props',[]):
            if 'mute' in prm: res=str(prm['mute']).lower()
print(res)"
}
check_t() { t1=$(title); sleep 2.5; t2=$(title); [ "$t1" != "$t2" ]; }
xdotool key ctrl+m; sleep 2; shot 02-muted
m=$(stream_mute)
if [ "$m" = true ] || wait_streams 0 4; then echo "ok   заглушённая вкладка: поток замьючен или исчез (mute=$m)"; else echo "FAIL после Ctrl+M звук не заглушён (mute=$m)"; fail=1; fi
if check_t; then echo "ok   ролик при этом идёт ($t1 → $t2)"; else echo "FAIL ролик встал при заглушении ($t1)"; fail=1; fi
xdotool key ctrl+m; sleep 2
m=$(stream_mute)
if [ "$m" = false ] || wait_streams ">0" 4; then echo "ok   звук включён обратно (mute=$m)"; else echo "FAIL звук не вернулся (mute=$m)"; fail=1; fi
gui_finish
