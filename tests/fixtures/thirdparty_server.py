#!/usr/bin/env python3
"""Сервер для gui-thirdparty.sh: два порта. Сайт (порт 1) отдаёт страницу с картинкой «трекера» с другого хоста (127.0.0.1:порт 2,
сайт открывают как localhost). Трекер ставит куку t=1 и дописывает в файл журнала заголовок Cookie каждого запроса.
Аргумент — путь журнала. Печатает порты сайта и трекера первой строкой."""
import http.server, socketserver, sys, threading

LOG = sys.argv[1]
socketserver.ThreadingTCPServer.daemon_threads = True

def tracker_handler():
    class T(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def do_GET(self):
            with open(LOG, "a") as f:
                f.write("cookie=%s\n" % (self.headers.get("Cookie") or "-"))
            pixel = bytes.fromhex("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000001e221bc330000000049454e44ae426082")
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Set-Cookie", "t=1; Path=/; Max-Age=3600")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(pixel)))
            self.end_headers(); self.wfile.write(pixel)
    return T

def site_handler(tracker_port):
    class S(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def do_GET(self):
            body = ('<!doctype html><meta charset=utf-8><title>tp-site</title><img src="http://127.0.0.1:%d/px">' % tracker_port).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body)
    return S

tracker = socketserver.ThreadingTCPServer(("127.0.0.1", 0), tracker_handler())
site = socketserver.ThreadingTCPServer(("127.0.0.1", 0), site_handler(tracker.server_address[1]))
threading.Thread(target=tracker.serve_forever, daemon=True).start()
print(site.server_address[1], tracker.server_address[1], flush=True)
site.serve_forever()
