#!/usr/bin/env bash
# Проверка подписанного канала обновлений (I14) на реальных процессах: ключи, манифест, подпись, `nrb update check/download`,
# подмена артефакта и манифеста, чужой ключ, откат версии и проверка бинаря в `nrb deploy`. Всё на loopback.
# Использование: scripts/update-e2e.sh [путь/к/nrb]
set -u
cd "$(dirname "$0")/.."
NRB="${1:-target/debug/nrb}"
DIR="$(mktemp -d)"; PORT="${NRB_E2E_PORT:-18975}"
fail=0
ok()  { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
cleanup() { kill ${WEB:-} 2>/dev/null; rm -rf "$DIR"; }
trap cleanup EXIT
mkdir -p "$DIR/www" "$DIR/keys" "$DIR/other" "$DIR/dl"

"$NRB" release keygen --out "$DIR/keys" > /dev/null && ok "keygen" || bad "keygen"
[ "$(stat -c %a "$DIR/keys/update.key")" = 600 ] && ok "секретный ключ с правами 0600" || bad "права update.key"
"$NRB" release keygen --out "$DIR/other" > /dev/null

ARCH="$(uname -m)"
head -c 20000 /dev/urandom > "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz"
"$NRB" release manifest --version 99.0.0 --notes "тестовый релиз" --artifact "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz" \
  --base-url "http://127.0.0.1:$PORT" -o "$DIR/www/manifest.json" > /dev/null && ok "манифест собран" || bad "manifest"
"$NRB" release sign "$DIR/www/manifest.json" --key "$DIR/keys/update.key" > /dev/null && ok "манифест подписан" || bad "sign"
"$NRB" release verify "$DIR/www/manifest.json" --pubkey "$DIR/keys/update.pub" --artifacts-dir "$DIR/www" > /dev/null && ok "verify: подпись и хеши верны" || bad "verify"
"$NRB" release verify "$DIR/www/manifest.json" --pubkey "$DIR/other/update.pub" > /dev/null 2>&1 && bad "чужой ключ принят" || ok "verify с чужим ключом отвергнут"

python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$DIR/www" > /dev/null 2>&1 &
WEB=$!
for _ in $(seq 1 30); do ss -tln | grep -q ":$PORT " && break; sleep 0.2; done
URL="http://127.0.0.1:$PORT/manifest.json"

"$NRB" update check --url "$URL" --pubkey "$DIR/keys/update.pub" 2>&1 | grep -q "доступна версия 99.0.0" && ok "update check видит новую версию" || bad "update check"
"$NRB" update download --url "$URL" --pubkey "$DIR/keys/update.pub" --out "$DIR/dl" > /dev/null 2>&1 \
  && cmp -s "$DIR/dl/nrb-99.0.0-linux-$ARCH.tar.gz" "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz" && ok "артефакт скачан, хеш проверен, содержимое совпало" || bad "update download"
"$NRB" update check --url "$URL" --pubkey "$DIR/other/update.pub" > /dev/null 2>&1 && bad "check принял манифест чужим ключом" || ok "check с чужим ключом отвергнут"

# Подмена артефакта на сервере: хеш не сходится — файл не пишется.
rm -rf "$DIR/dl"; mkdir "$DIR/dl"
cp "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz" "$DIR/artifact.orig"
head -c 20000 /dev/urandom > "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz"
"$NRB" update download --url "$URL" --pubkey "$DIR/keys/update.pub" --out "$DIR/dl" > "$DIR/tamper.log" 2>&1 && bad "подменённый артефакт принят" || ok "подменённый артефакт отвергнут"
grep -q "хеш" "$DIR/tamper.log" && ok "причина — несовпадение хеша" || bad "нет сообщения о хеше: $(cat "$DIR/tamper.log")"
[ -z "$(ls -A "$DIR/dl")" ] && ok "подменённый файл не записан на диск" || bad "файл записан"
cp "$DIR/artifact.orig" "$DIR/www/nrb-99.0.0-linux-$ARCH.tar.gz"

# Подмена манифеста: подпись не сходится.
cp "$DIR/www/manifest.json" "$DIR/manifest.orig"
sed -i 's/99\.0\.0/99.0.1/' "$DIR/www/manifest.json"
"$NRB" update check --url "$URL" --pubkey "$DIR/keys/update.pub" > "$DIR/m.log" 2>&1 && bad "изменённый манифест принят" || ok "изменённый манифест отвергнут"
grep -q "подпись манифеста неверна" "$DIR/m.log" && ok "причина — неверная подпись" || bad "сообщение: $(cat "$DIR/m.log")"
cp "$DIR/manifest.orig" "$DIR/www/manifest.json"

# Откат версии: валидно подписанный, но не новее установленной.
"$NRB" release manifest --version 0.0.0 --artifact "$DIR/artifact.orig" -o "$DIR/www/old.json" > /dev/null
"$NRB" release sign "$DIR/www/old.json" --key "$DIR/keys/update.key" > /dev/null
OLD="http://127.0.0.1:$PORT/old.json"
"$NRB" update check --url "$OLD" --pubkey "$DIR/keys/update.pub" 2>&1 | grep -q "самая новая" && ok "check: старая версия не предлагается" || bad "check старой версии"
"$NRB" update download --url "$OLD" --pubkey "$DIR/keys/update.pub" --artifact artifact.orig --out "$DIR/dl" > /dev/null 2>&1 && bad "откат принят" || ok "download: откат на старую версию запрещён"

# deploy: бинарь обязан входить в подписанный манифест (проверка идёт ДО подключения к серверу).
"$NRB" deploy root@127.0.0.1 --ssh-port 1 --binary "$DIR/artifact.orig" --manifest "$DIR/www/manifest.json" --pubkey "$DIR/keys/update.pub" > "$DIR/d1.log" 2>&1
grep -q "бинарь проверен по подписанному манифесту" "$DIR/d1.log" && ok "deploy: подписанный бинарь принят (дальше — подключение)" || bad "deploy подписанного: $(cat "$DIR/d1.log")"
head -c 20000 /dev/urandom > "$DIR/evil.bin"
"$NRB" deploy root@127.0.0.1 --ssh-port 1 --binary "$DIR/evil.bin" --manifest "$DIR/www/manifest.json" --pubkey "$DIR/keys/update.pub" > "$DIR/d2.log" 2>&1
grep -q "не входит в подписанный манифест" "$DIR/d2.log" && ! grep -q "подключаюсь" "$DIR/d2.log" && ok "deploy: чужой бинарь отвергнут до подключения к серверу" || bad "deploy чужого: $(cat "$DIR/d2.log")"

echo; [ "$fail" -eq 0 ] && echo "update-e2e: всё прошло" || echo "update-e2e: есть падения"
exit "$fail"
