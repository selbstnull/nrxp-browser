#!/usr/bin/env python3
"""Заглушка TURN-сервера для gui-webrtc.sh: принимает TCP-соединения и записывает в журнал (первый аргумент) «accept» и
первые байты. Печатает порт первой строкой. STUN/TURN не отвечает — проверяется только путь соединения."""
import socket, sys, threading
log = open(sys.argv[1], "a", buffering=1)
srv = socket.socket(); srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", 0)); srv.listen(16)
print(srv.getsockname()[1], flush=True)
def serve(conn, peer):
    log.write("accept %d\n" % peer[1])
    try:
        data = conn.recv(64)
        log.write("bytes %d first=%s\n" % (len(data), data[:2].hex()))
        while conn.recv(4096): pass
    except Exception: pass
while True:
    c, p = srv.accept()
    threading.Thread(target=serve, args=(c, p), daemon=True).start()
