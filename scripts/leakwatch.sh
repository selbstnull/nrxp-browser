#!/usr/bin/env bash
# Наблюдатель утечек: пока не появится файл-флаг $1, раз в 0,1 с смотрит сокеты процессов `nrb` и печатает уникальные
# не-loopback адреса (TCP и UDP, в т.ч. DNS). Пусто = ни одного соединения в обход туннеля.
# Работает, только если сам узел NRXP на loopback (dev-node.sh); с удалённым узлом в выводе будет его адрес — это ожидаемо.
# Использование:  scripts/leakwatch.sh /tmp/stop > leaks.txt &   …прогон…   touch /tmp/stop; wait
set -u
stop="${1:?нужен путь файла-флага остановки}"
rm -f "$stop"
while [ ! -e "$stop" ]; do
  ss -Htunp 2>/dev/null | grep '"nrb"' | awk '{print $1, $5, $6}' | grep -vE '127\.0\.0\.1|\[::1\]'
  sleep 0.1
done | sort -u
