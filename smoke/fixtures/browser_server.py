"""Bounded target-loopback server; live_browser injects only the public fixture."""
import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import time
from urllib.parse import urlsplit

body = base64.b64decode('__HTML_BASE64__', validate=True)


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        ok = urlsplit(self.path).path == '/browser.html'
        content = body if ok else b'not found'
        self.send_response(200 if ok else 404)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(content)))
        self.end_headers()
        self.wfile.write(content)

    def log_message(self, *_):
        pass


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
server.daemon_threads = True
server.timeout = .2
print(json.dumps({'port': server.server_port}), flush=True)
deadline = time.monotonic() + 600
try:
    while time.monotonic() < deadline:
        server.handle_request()
finally:
    server.server_close()
