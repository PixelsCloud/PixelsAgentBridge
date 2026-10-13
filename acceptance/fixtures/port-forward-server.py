"""Disposable loopback-only TCP echo and HTTP servers for Linux acceptance."""

import argparse
import http.server
import socket
import socketserver
import threading


class Echo(socketserver.BaseRequestHandler):
    def handle(self):
        while chunk := self.request.recv(16 * 1024):
            self.request.sendall(chunk)
        self.request.shutdown(socket.SHUT_WR)


class ThreadedTcpServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


class Web(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = f'pixels-port-forward:{self.path}'.encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/plain; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('echo_port', type=int)
parser.add_argument('http_port', type=int)
args = parser.parse_args()
echo = ThreadedTcpServer(('127.0.0.1', args.echo_port), Echo)
web = http.server.ThreadingHTTPServer(('127.0.0.1', args.http_port), Web)
threading.Thread(target=echo.serve_forever, daemon=True).start()
print(f'echo={args.echo_port} http={args.http_port}', flush=True)
web.serve_forever()
