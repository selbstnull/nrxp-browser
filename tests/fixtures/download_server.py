#!/usr/bin/env python3
"""Сервер для gui-downloads.sh: страница ставит куку sid=7, файл отдаётся только с этой кукой. Печатает порт первой строкой."""
import http.server, socketserver, sys

PAYLOAD = b"nrb-download-test-" * 2000  # ~36 КБ

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/page.html":
            body = '<!doctype html><meta charset=utf-8><title>dl-page</title><a href="/files/data.zip" style="font-size:48px">скачать</a>'.encode()
            self.send_response(200)
            self.send_header("Set-Cookie", "sid=7; Path=/")
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body)
        elif self.path == "/files/data.zip":
            if "sid=7" not in (self.headers.get("Cookie") or ""):
                self.send_response(403); self.send_header("Content-Length", "0"); self.end_headers(); return
            self.send_response(200)
            self.send_header("Content-Type", "application/zip")
            self.send_header("Content-Length", str(len(PAYLOAD)))
            self.end_headers(); self.wfile.write(PAYLOAD)
        else:
            self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()

with socketserver.TCPServer(("127.0.0.1", 0), H) as srv:
    print(srv.server_address[1], flush=True)
    srv.serve_forever()
