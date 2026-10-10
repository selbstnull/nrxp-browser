#!/usr/bin/env bash
# GUI-сценарий N9: WebRTC только как relay через туннель (сборка --features media, GStreamer с webrtcbin и libnice).
#  A) по умолчанию WebRTC выключен: у страницы нет RTCPeerConnection;
#  B) включён в настройках, без TURN-сервера у страницы: кандидатов нет (ни host, ни srflx), у процесса нет UDP-сокетов,
#     нет SSDP/UPnP (порт 1900), нет сокетов на адресах локальной сети — только loopback;
#  C) со своим узлом и TURN-сервером страницы (заглушка на loopback): TCP-соединение к TURN приходит от процесса узла, а не
#     от окна браузера — то есть через туннель;
#  D) в герметичном режиме WebRTC запрещён даже при включённой настройке.
set -u
. "$(dirname "$0")/lib-gui.sh"
gui_init out/gui-webrtc
NRB="${1:-target/debug/nrb}"
TMP="$(mktemp -d)"
trap 'kill $PID $XEPHYR $TURN $NODE 2>/dev/null; rm -rf "$TMP"' EXIT
URL="file://$PWD/tests/fixtures/webrtc.html"

# Сокеты процесса: не loopback-адреса (кроме unix) — утечка локальной сети.
foreign_sockets() {
  ss -tuanp 2>/dev/null | grep "pid=$PID," | grep -vE " (127\.0\.0\.1|\[::1\]|\*):[0-9]+ .*(127\.0\.0\.1|\[::1\])" | grep -E " (UNCONN|LISTEN|ESTAB)" | grep -vE "127\.0\.0\.1:|\[::1\]:" || true
}

# --- A: выключено
mkdir -p "$TMP/off"; printf 'adblock = false\n' > "$TMP/off/settings.toml"
gui_launch "$NRB" --profile "$TMP/off" "$URL"; sleep 3
check_wait "A: WebRTC выключен по умолчанию" "rtc: no-RTCPeerConnection" 8
kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1

# --- B: включён, без TURN
mkdir -p "$TMP/on"; printf 'adblock = false\nwebrtc = true\n' > "$TMP/on/settings.toml"
gui_launch "$NRB" --profile "$TMP/on" "$URL"; sleep 6
check_wait "B: сбор кандидатов завершён, их нет" "gathered cands=0" 10
t=$(title); [[ "$t" == *LEAK* ]] && { echo "FAIL B: утечка host/srflx-кандидатов: $t"; fail=1; } || echo "ok   B: host/srflx-кандидатов нет"
f=$(foreign_sockets)
if [ -z "$f" ]; then echo "ok   B: сокетов вне loopback у процесса нет (UDP, SSDP/1900, LAN)"; else echo "FAIL B: сокеты вне loopback:"; echo "$f" | head -5; fail=1; fi
kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1

# --- C: через узел, TURN-заглушка на loopback
python3 tests/fixtures/fake_turn.py "$TMP/turn.log" > "$TMP/turn.port" &
TURN=$!
for _ in $(seq 1 30); do [ -s "$TMP/turn.port" ] && break; sleep 0.2; done
TP=$(head -1 "$TMP/turn.port")
NODEPORT=$((TP + 1))
"$NRB" node init --dir "$TMP/node" --public-host 127.0.0.1 --port "$NODEPORT" > /dev/null
"$NRB" node run --dir "$TMP/node" > /dev/null 2>&1 &
NODE=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$NODEPORT " && break; sleep 0.25; done
LINK="$("$NRB" node invite --dir "$TMP/node" окно 2>/dev/null)"
CFG="{\"iceServers\":[{\"urls\":\"turn:127.0.0.1:$TP?transport=udp\",\"username\":\"u\",\"credential\":\"p\"}]}"
CFGQ=$(python3 -c "import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1]))" "$CFG")
gui_launch "$NRB" --profile "$TMP/on" --allow-private --node "$LINK" "$URL?cfg=$CFGQ"; sleep 8
for _ in $(seq 1 20); do grep -q accept "$TMP/turn.log" 2>/dev/null && break; sleep 0.5; done
if grep -q accept "$TMP/turn.log" 2>/dev/null; then echo "ok   C: TURN-сервер получил TCP-соединение ($(tr '\n' ' ' < "$TMP/turn.log"))"; else echo "FAIL C: соединения к TURN не было"; fail=1; fi
# Кто держит клиентский конец соединения к TURN: процесс узла или окно?
owners=$(ss -tnp dst "127.0.0.1:$TP" 2>/dev/null | grep -oE "pid=[0-9]+" | sort -u | tr '\n' ' ')
if [[ "$owners" == *"pid=$NODE"* ]] && [[ "$owners" != *"pid=$PID"* ]]; then echo "ok   C: к TURN ходит процесс узла ($owners), окно браузера — нет"; else echo "FAIL C: владельцы соединения: «$owners» (узел $NODE, окно $PID)"; fail=1; fi
# (у окна с туннелем есть свои UDP-сокеты к узлу — на loopback; чужих адресов быть не должно)
f=$(foreign_sockets)
if [ -z "$f" ]; then echo "ok   C: сокетов вне loopback у окна нет"; else echo "FAIL C: сокеты вне loopback:"; echo "$f" | head -5; fail=1; fi
kill $PID 2>/dev/null; wait $PID 2>/dev/null; sleep 1
# --- D: герметичный режим — WebRTC недоступен при любой настройке
gui_launch "$NRB" --profile "$TMP/on" --hermetic --allow-private --node "$LINK" "$URL"; sleep 6
check_wait "D: в герметичном режиме RTCPeerConnection нет, хотя настройка включена" "rtc: no-RTCPeerConnection" 10
gui_finish
