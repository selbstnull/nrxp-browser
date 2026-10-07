#!/usr/bin/env python3
"""Мини-сервер для проверки HTTP Basic-авторизации: логин user / пароль passwd. Печатает порт в stdout первой строкой."""
import base64, http.server, sys

EXPECTED = "Basic " + base64.b64encode(b"user:passwd").decode()

class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") == EXPECTED:
            body = b"<!doctype html><meta charset=utf-8><title>auth ok</title><p>auth ok</p>"
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
        else:
            body = b"<!doctype html><title>auth required</title>denied"
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="nrb-test"')
            self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
