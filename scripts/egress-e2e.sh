#!/usr/bin/env bash
# Сквозная проверка I9 (reverse-egress), три отдельных процесса `nrb` на loopback:
#   клиент (`nrb fetch`) → узел (`nrb node run`, exit = require) → домашний выход (`nrb egress`) → HTTP-сервер.
# Цель — HTTP-сервер на 127.0.0.1, поэтому выходу нужен --allow-lan; без него цель обязана быть недоступна.
# «Выход через дом, а не с узла» доказываем режимом require: с остановленным egress тот же запрос обязан падать.
# Использование: scripts/egress-e2e.sh [путь/к/nrb]
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"
DIR="$(mktemp -d)"; WEB="$(mktemp -d)"
PORT="${NRB_E2E_PORT:-18950}"; WEBPORT="${NRB_E2E_WEBPORT:-18951}"
fail=0
ok()  { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
cleanup() { for p in ${NODE_PID:-} ${EGRESS_PID:-} ${WEB_PID:-}; do kill "$p" 2>/dev/null; done; rm -rf "$DIR" "$WEB"; }
trap cleanup EXIT

echo "из дома через узел" > "$WEB/index.html"
python3 -m http.server "$WEBPORT" --bind 127.0.0.1 --directory "$WEB" > /dev/null 2>&1 &
WEB_PID=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$WEBPORT " && break; sleep 0.1; done
URL="http://127.0.0.1:$WEBPORT/index.html"

"$NRB" node init --dir "$DIR" --public-host 127.0.0.1 --port "$PORT" > /dev/null || { bad "node init"; exit 1; }
sed -i 's/^exit = .*/exit = "require"/' "$DIR/node.toml"
grep -q '^exit = "require"' "$DIR/node.toml" || echo 'exit = "require"' >> "$DIR/node.toml"
"$NRB" node run --dir "$DIR" > "$DIR/node.log" 2>&1 &
NODE_PID=$!
for _ in $(seq 1 40); do ss -tln | grep -q ":$PORT " && break; sleep 0.25; done
grep -q "ТОЛЬКО через домашний egress" "$DIR/node.log" && ok "узел в режиме require" || { bad "режим require не включился"; cat "$DIR/node.log"; exit 1; }

CLIENT="$("$NRB" node invite --dir "$DIR" телефон 2>/dev/null)"
EGRESS="$("$NRB" node invite --dir "$DIR" --egress дом 2>/dev/null)"
[ -n "$CLIENT" ] && [ -n "$EGRESS" ] && ok "выданы два приглашения (клиент и выход)" || { bad "приглашения"; exit 1; }
"$NRB" node devices --dir "$DIR" | grep -q "выход" && ok "devices показывает роль выхода" || bad "роль в devices"

fetch() { "$NRB" fetch "$URL" --node "$1" -o "$DIR/out.bin" --timeout "${2:-8s}" > /dev/null 2>&1; }

# 1. Выхода нет — клиент не получает даже доступную узлу цель (fail closed).
fetch "$CLIENT" && bad "без egress запрос прошёл (трафик вышел с самого узла!)" || ok "без egress запрос не проходит (fail closed)"

# 2. Роли не смешиваются.
fetch "$EGRESS" && bad "токен выхода сработал как клиентский" || ok "токен выхода не годится как клиентский"
"$NRB" egress "$CLIENT" --allow-lan > "$DIR/bad-egress.log" 2>&1 && bad "клиентский токен стал выходом" || ok "клиентский токен не даёт роль выхода"

# 3. Домашний выход по умолчанию не пускает в свою сеть.
"$NRB" egress "$EGRESS" > "$DIR/egress.log" 2>&1 &
EGRESS_PID=$!
for _ in $(seq 1 40); do grep -q "подключён" "$DIR/egress.log" && break; sleep 0.25; done
grep -q "запрещены" "$DIR/egress.log" && ok "egress подключён, LAN закрыт по умолчанию" || { bad "egress не подключился"; cat "$DIR/egress.log"; }
fetch "$CLIENT" && bad "через выход достижим loopback дома (политика не работает)" || ok "loopback/LAN выхода недоступен по умолчанию"
kill "$EGRESS_PID"; wait "$EGRESS_PID" 2>/dev/null

# 4. С явным --allow-lan — цель достижима, тело совпадает.
"$NRB" egress "$EGRESS" --allow-lan > "$DIR/egress.log" 2>&1 &
EGRESS_PID=$!
for _ in $(seq 1 40); do grep -q "подключён" "$DIR/egress.log" && break; sleep 0.25; done
grep -q "РАЗРЕШЕНЫ" "$DIR/egress.log" && ok "egress с --allow-lan подключён" || bad "egress с --allow-lan"
fetch "$CLIENT" 15s && cmp -s "$DIR/out.bin" "$WEB/index.html" && ok "клиент → узел → дом → цель: тело совпадает" || bad "запрос через дом не прошёл"

# 4a. Публичный сайт через дом (DNS разрешается на выходе, TLS — сквозной). Пропуск, если сети нет.
if curl -s --max-time 10 -o /dev/null https://example.org/; then
  "$NRB" fetch https://example.org/ --node "$CLIENT" -o "$DIR/pub.bin" --timeout 40s > /dev/null 2>&1 && grep -qi "example domain" "$DIR/pub.bin" && ok "https://example.org через дом ($(wc -c < "$DIR/pub.bin") байт)" || bad "публичный сайт через дом"
else
  echo "пропуск: нет выхода в интернет"
fi

# 5. Остановка выхода — клиенты снова без выхода, узел жив.
kill "$EGRESS_PID"; wait "$EGRESS_PID" 2>/dev/null; EGRESS_PID=""
sleep 2
fetch "$CLIENT" && bad "после остановки egress запрос всё ещё проходит" || ok "после остановки egress запросы не выходят с узла"
kill -0 "$NODE_PID" 2>/dev/null && ok "узел продолжает работать" || bad "узел упал"

# 5a. Выход по устройству и живая смена режима (без перезапуска узла); выхода сейчас нет, режим require.
"$NRB" node exit --dir "$DIR" телефон local > /dev/null
sleep 1
fetch "$CLIENT" 15s && ok "устройство с выходом «local» работает без домашнего выхода при require" || bad "exit local"
"$NRB" node exit --dir "$DIR" телефон default > /dev/null
sleep 1
fetch "$CLIENT" && bad "после сброса выхода устройство всё ещё идёт с узла" || ok "после сброса выхода снова отказ (require)"
"$NRB" node exit --dir "$DIR" телефон дом > /dev/null
sleep 1
fetch "$CLIENT" && bad "привязка к отсутствующей метке выпустила трафик" || ok "привязка к отсутствующей метке — отказ, без выхода с узла"
"$NRB" node exit --dir "$DIR" телефон default > /dev/null
"$NRB" node mode --dir "$DIR" local > /dev/null
sleep 4
fetch "$CLIENT" 15s && ok "режим local применён на лету (без перезапуска узла)" || bad "живая смена режима на local"
"$NRB" node mode --dir "$DIR" require > /dev/null
sleep 4
fetch "$CLIENT" && bad "режим require не вернулся" || ok "режим require вернулся на лету"
"$NRB" node mode --dir "$DIR" мусор > /dev/null 2>&1 && bad "принят неверный режим" || ok "неверный режим отвергнут командой"

# 6. Отзыв выхода на лету.
"$NRB" egress "$EGRESS" --allow-lan > "$DIR/egress.log" 2>&1 &
EGRESS_PID=$!
for _ in $(seq 1 40); do grep -q "подключён" "$DIR/egress.log" && break; sleep 0.25; done
fetch "$CLIENT" 15s && ok "переподключённый выход снова несёт трафик" || bad "повторное подключение выхода"
"$NRB" node revoke --dir "$DIR" дом > /dev/null
sleep 5
fetch "$CLIENT" && bad "отозванный выход ещё несёт трафик" || ok "отзыв убирает выход из маршрутов"
for _ in $(seq 1 20); do kill -0 "$EGRESS_PID" 2>/dev/null || break; sleep 0.25; done
kill -0 "$EGRESS_PID" 2>/dev/null && bad "процесс egress не завершился после отзыва" || ok "процесс egress завершился после отзыва"

echo; [ "$fail" -eq 0 ] && echo "egress-e2e: всё прошло" || echo "egress-e2e: есть падения"
exit "$fail"
