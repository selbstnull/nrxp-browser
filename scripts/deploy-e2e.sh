#!/usr/bin/env bash
# Сквозная проверка I8: `nrb deploy` на тестовый «сервер» — контейнер Ubuntu 24.04 с systemd и sshd (tests/deploy/Dockerfile).
# Настоящие серверы не затрагиваются. Нужны docker и release-бинарь nrb (по умолчанию target-release/release/nrb, при отсутствии собирается).
# Использование: scripts/deploy-e2e.sh        Переменные: NRB_NODE_BINARY, NRB_E2E_SSH_PORT (2222), NRB_E2E_NODE_PORT (18543)
set -u
cd "$(dirname "$0")/.."
BIN="${NRB_NODE_BINARY:-target-release/release/nrb}"
SSH_PORT="${NRB_E2E_SSH_PORT:-2222}"; NODE_PORT="${NRB_E2E_NODE_PORT:-18543}"
NAME=nrb-deploy-e2e
T="$(mktemp -d)"; fail=0
ok()  { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
cleanup() { docker rm -f "$NAME" >/dev/null 2>&1; rm -rf "$T"; }
[ -z "${NRB_E2E_KEEP:-}" ] && trap cleanup EXIT
command -v docker >/dev/null || { echo "нужен docker"; exit 2; }

if [ ! -x "$BIN" ]; then
  echo "собираю $BIN (release, без движка)…"
  CARGO_TARGET_DIR=target-release cargo build --release -p nrb 2>&1 | tail -1
fi
[ -x "$BIN" ] || { echo "нет бинаря $BIN"; exit 2; }
export XDG_DATA_HOME="$T/data"          # known_hosts.toml и прочее — в temp, не в настоящий профиль
NRB="$BIN"
C() { docker exec "$NAME" sh -c "$1"; }

docker build -q -t nrb-deploy-target tests/deploy >/dev/null || { echo "не собрался тестовый образ"; exit 2; }
docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" --cap-add SYS_ADMIN --security-opt apparmor=unconfined --cgroupns=host \
  -v /sys/fs/cgroup:/sys/fs/cgroup:rw --tmpfs /run --tmpfs /run/lock \
  -p "127.0.0.1:$SSH_PORT:22" -p "127.0.0.1:$NODE_PORT:$NODE_PORT/tcp" -p "127.0.0.1:$NODE_PORT:$NODE_PORT/udp" nrb-deploy-target >/dev/null
[ "$(docker inspect -f '{{.State.Running}}' "$NAME" 2>/dev/null)" = "true" ] || { echo "контейнер не запустился (порты $SSH_PORT/$NODE_PORT заняты?)"; exit 2; }
for _ in $(seq 1 40); do C 'systemctl is-active ssh' >/dev/null 2>&1 && break; sleep 0.5; done
ssh-keygen -q -t ed25519 -N '' -f "$T/id" >/dev/null
docker cp "$T/id.pub" "$NAME:/home/deploy/.ssh/authorized_keys"
C 'chown deploy:deploy /home/deploy/.ssh/authorized_keys && chmod 600 /home/deploy/.ssh/authorized_keys'
FP="$(C 'ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub' | awk '{print $2}')"
echo "тестовый сервер: 127.0.0.1:$SSH_PORT, отпечаток $FP"

DEPLOY=("$NRB" deploy "deploy@127.0.0.1" --ssh-port "$SSH_PORT" --key "$T/id" --node-port "$NODE_PORT" --public-host 127.0.0.1 --binary "$BIN")
installed() { C 'test -x /opt/nrb/nrb' 2>/dev/null; }

echo "== хост-ключ"
"${DEPLOY[@]}" </dev/null >"$T/o1" 2>"$T/e1" && bad "без доверия к ключу деплой прошёл" || { grep -q "не принят" "$T/e1" && ok "неизвестный ключ в неинтерактивном режиме не принят" || bad "сообщение об отказе: $(tail -2 "$T/e1")"; }
installed && bad "сервер изменён без доверия к ключу" || ok "сервер не тронут"
"${DEPLOY[@]}" --trust-host-key SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA </dev/null >/dev/null 2>"$T/e2" && bad "принят чужой отпечаток" || ok "неверный отпечаток отвергнут"
"$NRB" deploy "deploy@127.0.0.1" --ssh-port "$SSH_PORT" --key "$T/nonexistent" --binary "$BIN" --trust-host-key "$FP" </dev/null >/dev/null 2>"$T/e3" && bad "несуществующий ключ" || ok "несуществующий ключ — понятная ошибка"

echo "== dry-run"
"${DEPLOY[@]}" --trust-host-key "$FP" --dry-run </dev/null >/dev/null 2>"$T/e4" && ok "dry-run прошёл" || bad "dry-run: $(tail -2 "$T/e4")"
installed && bad "dry-run изменил сервер" || ok "dry-run ничего не поставил"

echo "== установка"
"${DEPLOY[@]}" --trust-host-key "$FP" --device первое </dev/null >"$T/link1" 2>"$T/e5" && ok "деплой завершился успешно" || { bad "деплой: $(tail -4 "$T/e5")"; cat "$T/e5"; }
LINK1="$(grep '^nrxp://' "$T/link1" | head -1)"
[ -n "$LINK1" ] && ok "приглашение получено" || bad "нет приглашения"
C 'systemctl is-active nrb-node' | grep -q '^active' && ok "служба nrb-node активна" || bad "служба не активна"
C "ss -tln | grep -q ':$NODE_PORT '" && ok "узел слушает :$NODE_PORT" || bad "порт не слушается"
C 'ps -o user= -C nrb | head -1' | grep -q '^nrb-node' && ok "процесс идёт от пользователя nrb-node (не root)" || bad "процесс не от nrb-node: $(C 'ps -o user=,args= -C nrb' | head -2)"
[ "$(C 'stat -c %a:%U /var/lib/nrb-node/node.toml')" = "600:nrb-node" ] && ok "node.toml: 0600, владелец nrb-node" || bad "права node.toml: $(C 'stat -c %a:%U /var/lib/nrb-node/node.toml')"
C 'grep -q NoNewPrivileges=true /etc/systemd/system/nrb-node.service && systemctl show nrb-node -p NoNewPrivileges | grep -q yes' && ok "юнит усилен и применён (NoNewPrivileges)" || bad "юнит не усилен"
C 'systemctl is-enabled nrb-node' | grep -q enabled && ok "автозапуск включён" || bad "нет автозапуска"
[ -s "$XDG_DATA_HOME/netrunner-browser/known_hosts.toml" ] && ok "отпечаток сервера запомнен" || bad "пин не записан"
grep -q "$(echo "$LINK1" | sed -n 's/.*secret=\([0-9a-f]*\).*/\1/p')" "$T/e5" && bad "секрет узла попал в журнал деплоя" || ok "в журнале деплоя нет секрета узла"

echo "== трафик через развёрнутый узел"
"$NRB" fetch https://example.org/ --node "$LINK1" -o "$T/via.bin" --timeout 40s 2>/dev/null; curl -sL --max-time 30 -A "nrb/0.0.1" -H "Accept-Encoding: identity" https://example.org/ -o "$T/direct.bin"
cmp -s "$T/via.bin" "$T/direct.bin" && ok "fetch через сервер == прямой curl ($(wc -c <"$T/via.bin") байт)" || bad "тело через развёрнутый узел отличается"

echo "== апгрейд (повторный запуск)"
PK1="$(echo "$LINK1" | sed -n 's/.*pk=\([0-9a-f]*\).*/\1/p')"
SHA_BEFORE="$(C 'sha256sum /opt/nrb/nrb' | cut -d' ' -f1)"
"${DEPLOY[@]}" --device второе </dev/null >"$T/link2" 2>"$T/e6" && ok "повторный деплой (пин без флага) прошёл" || { bad "повторный деплой"; sed 's/^/     | /' "$T/e6"; }
LINK2="$(grep '^nrxp://' "$T/link2" | head -1)"
[ "$(echo "$LINK2" | sed -n 's/.*pk=\([0-9a-f]*\).*/\1/p')" = "$PK1" ] && ok "ключи узла сохранены при апгрейде" || bad "ключи узла изменились"
"$NRB" fetch https://example.org/ --node "$LINK1" -o "$T/via1.bin" --timeout 40s 2>/dev/null && cmp -s "$T/via1.bin" "$T/direct.bin" && ok "устройство из первого деплоя работает после апгрейда" || bad "старое устройство сломалось"

echo "== откат при сбое проверки (апгрейд)"
"${DEPLOY[@]}" --device третье --inject-verify-failure </dev/null >/dev/null 2>"$T/e7" && bad "искусственный сбой не сработал" || ok "деплой с искусственным сбоем завершился ошибкой"
grep -q "откат выполнен" "$T/e7" && ok "откат выполнен" || { bad "нет отката"; sed 's/^/     | /' "$T/e7"; }
[ "$(C 'sha256sum /opt/nrb/nrb' | cut -d' ' -f1)" = "$SHA_BEFORE" ] && ok "бинарь прежний" || bad "бинарь после отката отличается"
C 'systemctl is-active nrb-node' | grep -q '^active' && ok "служба работает после отката" || bad "служба лежит после отката"
"$NRB" fetch https://example.org/ --node "$LINK1" -o "$T/via2.bin" --timeout 40s 2>/dev/null && cmp -s "$T/via2.bin" "$T/direct.bin" && ok "трафик идёт после отката" || bad "трафик не идёт после отката"

echo "== удаление"
"$NRB" deploy "deploy@127.0.0.1" --ssh-port "$SSH_PORT" --key "$T/id" --node-port "$NODE_PORT" --remove </dev/null >/dev/null 2>"$T/e8" && ok "--remove прошёл" || bad "--remove: $(tail -2 "$T/e8")"
C 'test ! -e /opt/nrb && test ! -e /var/lib/nrb-node && test ! -e /etc/systemd/system/nrb-node.service && ! id nrb-node 2>/dev/null' && ok "бинарь, данные, юнит и пользователь удалены" || bad "после удаления остались следы"
C "ss -tln | grep -q ':$NODE_PORT '" && bad "порт всё ещё слушается" || ok "порт закрыт"

echo "== откат при сбое проверки (чистая установка)"
"${DEPLOY[@]}" --device четвёртое --inject-verify-failure </dev/null >/dev/null 2>"$T/e9" && bad "искусственный сбой не сработал" || ok "ошибка при искусственном сбое"
C 'test ! -e /opt/nrb && test ! -e /var/lib/nrb-node && test ! -e /etc/systemd/system/nrb-node.service && ! id nrb-node 2>/dev/null' && ok "после отката чистой установки следов нет" || bad "остались следы: $(C 'ls /opt /var/lib | tr "\n" " "; id nrb-node 2>&1 | head -1')"

echo; [ "$fail" -eq 0 ] && echo "deploy-e2e: всё прошло" || echo "deploy-e2e: есть падения"
exit "$fail"
