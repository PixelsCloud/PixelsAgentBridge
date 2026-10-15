"""Exercise the native server-probe example over isolated HTTPS/WSS fixtures.

Requires Python cryptography (test certificate generation only).
Build: cargo build -p pab-agent-core --example server-probe -j 10
Run: python scripts/test_server_probe.py
"""
import base64
from datetime import datetime, timedelta, timezone
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ipaddress
import json
from pathlib import Path
import ssl
import struct
import subprocess
import tempfile
import threading
import time
import unittest

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/debug/examples' / ('server-probe.exe' if __import__('os').name == 'nt' else 'server-probe')


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *_):
        pass

    def do_GET(self):
        self.close_connection = True
        self.server.requests.append((self.path, dict(self.headers)))
        mode = self.server.mode
        if mode == 'timeout':
            time.sleep(11)
            return
        if mode == 'redirect':
            self.send_response(302)
            self.send_header('Location', '/redirect-target')
            self.end_headers()
            return
        if mode == 'unhealthy':
            self.send_response(503)
            self.end_headers()
            return
        if self.path == '/health':
            self.send_response(204)
            self.end_headers()
            return
        if self.path == '/healthz':
            body = b'<html>not a relay</html>' if mode == 'website' else json.dumps({'status': 'ok', 'version': '1.2.0'}).encode()
            if mode == 'oversized':
                body = b'x' * 5000
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        if self.path not in ('/control', '/relay'):
            self.send_response(404)
            self.end_headers()
            return
        key = self.headers['Sec-WebSocket-Key']
        accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        self.send_response(101)
        self.send_header('Upgrade', 'websocket')
        self.send_header('Connection', 'Upgrade')
        self.send_header('Sec-WebSocket-Accept', accept)
        if self.path == '/relay' and mode != 'wrong_protocol':
            self.send_header('Sec-WebSocket-Protocol', 'iroh-relay-v2')
        self.end_headers()
        if self.path == '/relay':
            return
        self.connection.settimeout(2)
        head = self.rfile.read(2)
        if len(head) != 2:
            return
        length = head[1] & 127
        if length == 126:
            length = struct.unpack('!H', self.rfile.read(2))[0]
        mask = self.rfile.read(4)
        raw = self.rfile.read(length)
        message = json.loads(bytes(value ^ mask[i % 4] for i, value in enumerate(raw)))
        assert message['type'] == 'list_traffic_scopes', message
        reply = {'type': 'error', 'request_id': message['request_id'], 'code': 'invalid_state', 'message': 'account login is required'}
        if mode == 'wrong_response':
            reply['request_id'] = '00000000-0000-0000-0000-000000000000'
        body = json.dumps(reply).encode()
        header = bytes([0x81, len(body)]) if len(body) < 126 else bytes([0x81, 126]) + struct.pack('!H', len(body))
        self.wfile.write(header + body)


class ProbeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix='pab-server-probe-')
        root = Path(cls.directory.name)
        key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
        now = datetime.now(timezone.utc)
        cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
                .serial_number(x509.random_serial_number()).not_valid_before(now - timedelta(days=1))
                .not_valid_after(now + timedelta(days=1))
                .add_extension(x509.SubjectAlternativeName([x509.IPAddress(ipaddress.ip_address('127.0.0.1'))]), critical=False)
                .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True).sign(key, hashes.SHA256()))
        cls.cert = root / 'cert.pem'
        cls.cert.write_bytes(cert.public_bytes(serialization.Encoding.PEM))
        private = root / 'key.pem'
        private.write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cls.cert, private)
        cls.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        cls.server.socket = context.wrap_socket(cls.server.socket, server_side=True)
        cls.server.daemon_threads = True
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.directory.cleanup()

    def run_probe(self, kind, mode='ok', trust=True):
        self.server.mode = mode
        self.server.requests = []
        scheme, path = ('wss', '/control') if kind == 'control' else ('https', '')
        url = f'{scheme}://127.0.0.1:{self.server.server_port}{path}'
        result = subprocess.run([str(BINARY), kind, url, *([str(self.cert)] if trust else [])], capture_output=True, text=True, timeout=15)
        value = json.loads(result.stdout)
        for _, headers in self.server.requests:
            self.assertFalse(any(key.lower() in ('cookie', 'authorization') for key in headers))
        return value

    def test_control_requires_a_correlated_protocol_reply(self):
        self.assertIn('elapsedMs', self.run_probe('control'))
        self.assertEqual(self.run_probe('control', 'wrong_response'), 'unexpected_response')

    def test_relay_requires_health_and_negotiated_relay_protocol(self):
        self.assertIn('elapsedMs', self.run_probe('relay'))
        self.assertEqual(self.run_probe('relay', 'wrong_protocol'), 'unexpected_response')

    def test_arbitrary_websites_and_large_responses_fail(self):
        for mode in ('website', 'oversized'):
            self.assertEqual(self.run_probe('relay', mode), 'unexpected_response')

    def test_redirects_are_not_followed(self):
        self.assertEqual(self.run_probe('control', 'redirect'), 'unexpected_response')
        self.assertEqual(len(self.server.requests), 1)

    def test_service_and_certificate_failures_are_distinct(self):
        self.assertEqual(self.run_probe('control', 'unhealthy'), 'unhealthy')
        self.assertEqual(self.run_probe('control', trust=False), 'tls')

    def test_hanging_service_has_a_deadline(self):
        self.assertEqual(self.run_probe('control', 'timeout'), 'timeout')


if __name__ == '__main__':
    unittest.main()
