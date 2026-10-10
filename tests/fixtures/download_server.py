#!/usr/bin/env python3
"""Сервер для gui-downloads.sh: страница ставит куку sid=7, файл отдаётся только с этой кукой. Печатает порт первой строкой."""
import http.server, socketserver, sys

PAYLOAD = b"nrb-download-test-" * 2000  # ~36 КБ
CSV = b"a,b\n1,2\n"

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        if self.path == "/page.html":
            body = ('<!doctype html><meta charset=utf-8><title>dl-page</title><a href="/files/data.zip" style="font-size:48px">скачать</a>'
                    '<a href="/export?id=5" style="position:absolute;left:0;top:200px;font-size:48px">экспорт</a>'
                    '<form method="post" action="/report" style="position:absolute;left:0;top:300px"><button style="font-size:48px">отчёт</button></form>').encode()
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
        elif self.path.startswith("/export"):
            # Динамический эндпоинт: имя только в Content-Disposition (русское, filename*), расширения в адресе нет.
            if "sid=7" not in (self.headers.get("Cookie") or ""):
                self.send_response(403); self.send_header("Content-Length", "0"); self.end_headers(); return
            self.send_attachment(CSV, "text/csv", "attachment; filename*=UTF-8''%D0%BE%D1%82%D1%87%D1%91%D1%82.csv")
        else:
            self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()

    def send_attachment(self, body, mime, disposition):
        self.send_response(200)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Disposition", disposition)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        self.rfile.read(length)
        if self.path == "/report":
            self.send_attachment(b"post-report-body", "application/octet-stream", "attachment; filename=report.bin")
        else:
            self.send_response(404); self.send_header("Content-Length", "0"); self.end_headers()

socketserver.ThreadingTCPServer.daemon_threads = True
with socketserver.ThreadingTCPServer(("127.0.0.1", 0), H) as srv:
    print(srv.server_address[1], flush=True)
    srv.serve_forever()
