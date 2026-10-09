#!/usr/bin/env python3
"""Сервер для gui-sitedata.sh: страница читает, что у неё уже есть (кука sid и localStorage.k), показывает это в заголовке и только потом ставит их скриптом."""
import http.server, socketserver

PAGE = """<!doctype html><meta charset=utf-8><title>sd: ?</title>
<script>
  var hadCookie = document.cookie.indexOf('sid=1') >= 0;
  var hadLs = localStorage.getItem('k') === 'v';
  document.title = 'sd: cookie=' + hadCookie + ' ls=' + hadLs;
  localStorage.setItem('k', 'v');
  document.cookie = 'sid=1; Path=/; Max-Age=3600';
</script>"""

class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        body = PAGE.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)

with socketserver.TCPServer(("127.0.0.1", 0), H) as srv:
    print(srv.server_address[1], flush=True)
    srv.serve_forever()
