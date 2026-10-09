"""Run real account/Executor acceptance with isolated PostgreSQL and loopback TLS.

The caller creates a database whose name starts with p1_. No installed service,
production database, or existing client profile is modified.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
from urllib.parse import urlsplit
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--database-env', default='P1_DATABASE_URL')
parser.add_argument('--openssl', default='openssl')
parser.add_argument('--mcp-count', type=int, default=2)
parser.add_argument('--relay-only', action='store_true')
parser.add_argument('--output', type=Path, default=Path('.build/p1-full-acceptance'))
args = parser.parse_args()
database = os.environ[args.database_env]
assert urlsplit(database).path.removeprefix('/').startswith('p1_'), 'isolated p1_ database required'
root = args.output.resolve() / uuid.uuid4().hex
root.mkdir(parents=True)
cert, key = root / 'cert.pem', root / 'key.pem'
subprocess.run([args.openssl, 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(key),
                '-out', str(cert), '-days', '2', '-subj', '/CN=localhost',
                '-addext', 'subjectAltName=DNS:localhost,IP:127.0.0.1', '-addext', 'basicConstraints=critical,CA:FALSE'],
               check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    port = listener.getsockname()[1]
origin = f'https://localhost:{port}'
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    relay_port = listener.getsockname()[1]
env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
env.update(PAB_DATABASE_URL=database, PAB_LISTEN_ADDR=f'127.0.0.1:{port}',
           PAB_TLS_CERT=str(cert), PAB_TLS_KEY=str(key), PAB_WEB_ORIGIN=origin,
           PAB_RELAY_CONTROL_SECRET=uuid.uuid4().hex, PAB_WEB_DIR=str(root / 'no-web-assets'))
server = str((args.binaries / ('pab-server.exe' if os.name == 'nt' else 'pab-server')).resolve())
relay = str((args.binaries / ('pab-relay-server.exe' if os.name == 'nt' else 'pab-relay-server')).resolve())
relay_process = None
with (root / 'server.log').open('w', encoding='utf-8') as log:
    subprocess.run([server, 'init'], env=env, stdout=log, stderr=log, check=True)
    process = subprocess.Popen([server, 'serve'], env=env, stdout=log, stderr=log,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
    try:
        for _ in range(100):
            assert process.poll() is None, f'isolated Server exited: {root / "server.log"}'
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=.5):
                    break
            except OSError:
                time.sleep(.1)
        relay_env = dict(env, PAB_CONTROL_URL=origin.replace('https:', 'wss:') + '/relay-control',
                         PAB_CONTROL_CA_CERT=str(cert), PAB_RELAY_TLS_CERT=str(cert), PAB_RELAY_TLS_KEY=str(key),
                         PAB_RELAY_HTTPS_ADDR=f'127.0.0.1:{relay_port}', PAB_RELAY_QUIC_ADDR='127.0.0.1:0',
                         PAB_RELAY_USAGE_DIR=str(root / 'relay-usage'), PAB_RELAY_NODE_ID='isolated-acceptance')
        relay_process = subprocess.Popen([relay], env=relay_env, cwd=root, stdout=log, stderr=log,
                                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        for _ in range(100):
            assert relay_process.poll() is None, f'isolated Relay exited: {root / "server.log"}'
            try:
                with socket.create_connection(('127.0.0.1', relay_port), timeout=.5):
                    break
            except OSError:
                time.sleep(.1)
        subprocess.run([sys.executable, str(Path(__file__).with_name('test_personal_account.py')),
                        '--binaries', str(args.binaries.resolve()), '--certificate', str(cert),
                        '--origin', origin, '--relay-url', f'https://localhost:{relay_port}',
                        '--mcp-count', str(args.mcp_count), '--output', str(root / 'clients'),
                        *(['--relay-only'] if args.relay_only else [])], check=True)
        print(json.dumps({'passed': True, 'evidence': str(root), 'server_pid': process.pid}))
    finally:
        if relay_process and relay_process.poll() is None:
            relay_process.terminate()
            relay_process.wait(timeout=20)
        process.terminate()
        process.wait(timeout=20)
