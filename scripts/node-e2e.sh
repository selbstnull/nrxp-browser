#!/usr/bin/env bash
# Сквозная проверка I7 без бэкенда: свой узел (nrb node), приглашение, `nrb pair`, трафик через узел, отзыв устройства.
# Всё на loopback. Использование: scripts/node-e2e.sh [путь/к/nrb]
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"
DIR="$(mktemp -d)"; HOME_DIR="$(mktemp -d)"
PORT="${NRB_E2E_PORT:-18900}"
fail=0
ok()   { echo "ok   $1"; }
bad()  { echo "FAIL $1"; fail=1; }
cleanup() { [ -n "${NODE_PID:-}" ] && kill "$NODE_PID" 2>/dev/null; rm -rf "$DIR" "$HOME_DIR"; }
trap cleanup EXIT
export XDG_DATA_HOME="$HOME_DIR"   # `nrb pair` пишет сюда, а не в настоящий профиль

"$NRB" node init --dir "$DIR" --public-host 127.0.0.1 --port "$PORT" > /dev/null && ok "node init" || bad "node init"
[ "$(stat -c %a "$DIR/node.toml")" = "600" ] && ok "node.toml с правами 0600" || bad "права node.toml"
"$NRB" node init --dir "$DIR" --public-host 127.0.0.1 --port "$PORT" > /dev/null 2>&1 && bad "повторный init затёр секреты" || ok "повторный init отказан"

"$NRB" node run --dir "$DIR" > "$DIR/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && break; sleep 0.25; done
ss -tln | grep -q ":$PORT " && ok "узел слушает :$PORT" || { bad "узел не поднялся"; cat "$DIR/node.log"; exit 1; }

LINK="$("$NRB" node invite --dir "$DIR" телефон 2>/dev/null)"
[[ "$LINK" == nrxp://127.0.0.1:$PORT\?* ]] && ok "приглашение выдано" || bad "приглашение: $LINK"
grep -q "$(echo "$LINK" | sed -n 's/.*token=\([0-9a-f]*\).*/\1/p')" "$DIR/devices.json" && bad "токен лежит в devices.json открытым текстом" || ok "в devices.json только хеш токена"

"$NRB" pair "$LINK" > "$DIR/pair.log" 2>&1 && ok "nrb pair" || bad "nrb pair: $(cat "$DIR/pair.log")"
CONF="$XDG_DATA_HOME/netrunner-browser/nodes/телефон.toml"
[ -f "$CONF" ] && [ "$(stat -c %a "$CONF")" = "600" ] && ok "конфиг спаривания (0600)" || bad "конфиг спаривания: $CONF"

# Диагностика: время первого подключения к свежему узлу (ключи — по конфигу, затем по ссылке).
for how in "$CONF" "$LINK"; do
  start=$(date +%s.%N)
  "$NRB" fetch https://example.org/ --node "$how" -o /dev/null --timeout 40s > /dev/null 2>"$DIR/timing.err" && res=ok || res=ОШИБКА
  [ "$res" = ОШИБКА ] && sed -n 1,3p "$DIR/timing.err" | cut -c1-200
  echo "     подключение через ${how:0:24}…: $res за $(echo "$(date +%s.%N) - $start" | bc) с"
done

# Трафик: сайт через узел, тело совпадает с прямой загрузкой.
URL="https://example.org/"
"$NRB" fetch "$URL" --node "$CONF" -o "$DIR/via.bin" --timeout 40s 2> "$DIR/via.err"; curl -sL --max-time 30 -A "nrb/0.0.1" -H "Accept-Encoding: identity" "$URL" -o "$DIR/direct.bin"
cmp -s "$DIR/via.bin" "$DIR/direct.bin" && ok "fetch через узел == прямой curl ($(wc -c < "$DIR/via.bin") байт)" || { bad "тело через узел отличается"; tail -3 "$DIR/via.err"; ls -l "$DIR/via.bin" "$DIR/direct.bin"; }
"$NRB" fetch "$URL" --node "$LINK" -o "$DIR/via2.bin" --timeout 40s 2>/dev/null && cmp -s "$DIR/via2.bin" "$DIR/direct.bin" && ok "--node принимает ссылку-приглашение" || bad "--node со ссылкой"

# Чужой клиент: тот же узел, но без токена из реестра.
BADLINK="$(echo "$LINK" | sed 's/token=[0-9a-f]*/token=0000000000000000000000000000000000000000000000000000000000000000/')"
"$NRB" fetch "$URL" --node "$BADLINK" -o "$DIR/bad.bin" --timeout 8s > /dev/null 2>&1 && bad "чужой токен получил доступ" || ok "чужой токен отвергнут"

# Сканер: обычный TLS-клиент (не знающий протокола) должен увидеть настоящий сайт decoy, а не прокси.
if command -v openssl >/dev/null; then
  CERT="$(echo | timeout 15 openssl s_client -connect "127.0.0.1:$PORT" -servername www.debian.org 2>/dev/null | openssl x509 -noout -subject 2>/dev/null)"
  [[ "$CERT" == *debian* ]] && ok "сканер видит decoy (сертификат: ${CERT#subject=})" || bad "сканер не увидел decoy (получено: «$CERT»)"
else
  echo "пропуск: нет openssl для проверки decoy"
fi

# Браузерный движок через свой узел (только если nrb собран с --features servo).
"$NRB" snap https://example.org/ -o "$DIR/shot.png" --node "$LINK" --timeout 40s > "$DIR/snap.log" 2>&1; code=$?
if [ "$code" -eq 64 ]; then echo "пропуск: nrb собран без движка (snap через узел не проверен)"
elif [ "$code" -eq 0 ]; then ok "snap страницы через свой узел"
else bad "snap через узел: код $code ($(tail -1 "$DIR/snap.log" | cut -c1-120))"; fi

# Отзыв.
"$NRB" node revoke --dir "$DIR" телефон > /dev/null && ok "revoke" || bad "revoke"
sleep 4
"$NRB" fetch "$URL" --node "$CONF" -o "$DIR/revoked.bin" --timeout 8s > /dev/null 2>&1 && bad "отозванное устройство всё ещё работает" || ok "отозванное устройство отвергнуто"

kill "$NODE_PID"; wait "$NODE_PID" 2>/dev/null; NODE_PID=""
grep -q "останавливаюсь" "$DIR/node.log" && ok "узел остановился штатно" || bad "нет штатной остановки"
echo; [ "$fail" -eq 0 ] && echo "node-e2e: всё прошло" || echo "node-e2e: есть падения"
exit "$fail"
