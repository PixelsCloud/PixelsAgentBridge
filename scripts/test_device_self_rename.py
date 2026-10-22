#!/usr/bin/env python3
"""Test the built server over verified TLS and signed device API requests.

Dependencies: cryptography, websocket-client==1.8.0, blake3==1.0.8.
Test data lives in the task's isolated, ephemeral PostgreSQL container.
"""
from __future__ import annotations
import datetime as dt
import json
from pathlib import Path
import socket
import ssl
import struct
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
WORK = ROOT / '.build/android/live-updates'
sys.path.insert(0, str(WORK/'python'))
from blake3 import blake3
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ed25519, rsa
from cryptography.x509.oid import NameOID
import websocket
from service_config import write_config


def command(*args):
    p = subprocess.run(args, capture_output=True, text=True, encoding='utf-8')
    if p.returncode:
        raise RuntimeError(p.stderr[-900:] or p.stdout[-900:])
    return p.stdout.strip()


def signed(challenge, key):
    principal = challenge['principal']
    prefix = {'user': 1, 'device': 2, 'guest': 3}[principal['kind']]
    context = struct.pack('>H', challenge['schema_version'])
    context += uuid.UUID(challenge['challenge_id']).bytes + uuid.UUID(challenge['connection_id']).bytes
    context += bytes([prefix])
    if prefix != 3:
        context += uuid.UUID(principal['user_id' if prefix == 1 else 'device_id']).bytes
    context += uuid.UUID(challenge['tenant_id']).bytes + bytes(challenge['endpoint_key'])
    context += bytes([{'register_user_endpoint': 1, 'register_device': 2,
        'authenticate_registered_endpoint': 3, 'register_unclaimed_device': 4,
        'register_guest_endpoint': 5}[challenge['purpose']]])
    context += struct.pack('>qq', challenge['issued_at_unix_ms'], challenge['expires_at_unix_ms'])
    context += bytes(challenge['nonce'])
    signature = key.sign(blake3(context, derive_key_context='pixels-agent-bridge endpoint proof v3').digest())
    return {'challenge_id': challenge['challenge_id'], 'signature': [list(signature[:32]), list(signature[32:])]}


def main():
    run = WORK / ('api-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    result = {'status': 'failed', 'checks': []}
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        port = listener.getsockname()[1]
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
    now = dt.datetime.now(dt.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
        .serial_number(x509.random_serial_number()).not_valid_before(now-dt.timedelta(minutes=1))
        .not_valid_after(now+dt.timedelta(days=1)).add_extension(x509.SubjectAlternativeName([x509.DNSName('localhost')]), False)
        .add_extension(x509.BasicConstraints(ca=True, path_length=None), True).sign(key, hashes.SHA256()))
    (run/'cert.pem').write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    (run/'key.pem').write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    (run/'web').mkdir()
    (run/'web/index.html').write_text('API validation', encoding='utf-8')
    remote = '/src/' + run.relative_to(ROOT).as_posix()
    write_config(run/'server.toml', {'listen': '0.0.0.0:18443',
        'database': {'url': 'postgres://postgres:test-only-local-android-name@pab-android-name-test-db/postgres'},
        'tls': {'cert': remote+'/cert.pem', 'key': remote+'/key.pem'},
        'web': {'origin': f'https://localhost:{port}', 'registration_enabled': True, 'assets': remote+'/web'},
        'relay': {'control_secret': 'isolated-local-test-relay-secret-0123456789'},
        'log': {'directory': remote+'/logs', 'level': 'warn'}})
    image = 'pab-linux-headless-build-deps:bullseye'
    container = 'pab-android-name-wire-api'
    common = ['--rm', '--network', 'pab-android-name-tests', '--cpus', '2', '--mount', f'type=bind,source={ROOT},target=/src']
    sockets = []
    started = False

    def passed(label):
        result['checks'].append(label)
        print(json.dumps({'check': label, 'passed': True}), flush=True)

    def connect():
        ws = websocket.create_connection(f'wss://localhost:{port}/control', timeout=8,
            sslopt={'ca_certs': str(run/'cert.pem'), 'cert_reqs': ssl.CERT_REQUIRED, 'check_hostname': True},
            origin=f'https://localhost:{port}', http_no_proxy=['localhost', '127.0.0.1'])
        sockets.append(ws)
        return ws

    def rpc(ws, kind, **fields):
        request = str(uuid.uuid4())
        ws.send(json.dumps({'type': kind, 'request_id': request, **fields}))
        reply = json.loads(ws.recv())
        assert reply.get('request_id') == request, reply
        return reply

    def register(guest=False):
        signing = ed25519.Ed25519PrivateKey.generate()
        endpoint = list(signing.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw))
        ws = connect()
        fields = {'endpoint_key': endpoint}
        if not guest:
            fields['name'] = 'Original phone'
        challenge = rpc(ws, 'begin_guest_endpoint_registration' if guest else 'begin_unclaimed_device_registration', **fields)
        assert challenge['type'] == 'endpoint_challenge', challenge
        registered = rpc(ws, 'complete_endpoint_registration', proof=signed(challenge['challenge'], signing))
        assert registered['type'] == 'endpoint_registered', registered
        return signing, registered['result']

    def authenticate(signing):
        ws = connect()
        endpoint = list(signing.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw))
        challenge = rpc(ws, 'begin_endpoint_authentication', endpoint_key=endpoint)
        assert challenge['type'] == 'endpoint_challenge', challenge
        assert rpc(ws, 'complete_endpoint_authentication', proof=signed(challenge['challenge'], signing))['type'] == 'endpoint_authenticated'
        return ws

    try:
        command('docker', 'run', *common, image, '/src/.build/linux-target/debug/pab-server', 'init', '--config', remote+'/server.toml')
        command('docker', 'run', '-d', '--name', container, *common, '-p', f'127.0.0.1:{port}:18443', image,
            '/src/.build/linux-target/debug/pab-server', 'serve', '--config', remote+'/server.toml')
        started = True
        for _ in range(60):
            try:
                anonymous = connect()
                break
            except (ConnectionError, OSError, websocket.WebSocketException):
                time.sleep(.3)
        else:
            raise RuntimeError('Isolated TLS backend did not start')
        assert rpc(anonymous, 'rename_own_device', name='Forbidden')['type'] == 'error'
        passed('unauthenticated_rename_rejected')
        signing, registered = register()
        first, second = authenticate(signing), authenticate(signing)
        assert rpc(first, 'rename_own_device', name='Renamed phone')['type'] == 'own_device_renamed'
        assert rpc(second, 'rename_own_device', name='Too soon')['code'] == 'rate_limited'
        assert rpc(second, 'rename_own_device', name='Renamed phone')['type'] == 'own_device_renamed'
        passed('signed_device_rename_and_cross_connection_cooldown')
        assert rpc(first, 'rename_own_device', name='\n')['type'] == 'error'
        guest, _ = register(guest=True)
        assert rpc(authenticate(guest), 'rename_own_device', name='Guest rename')['type'] == 'error'
        passed('invalid_names_and_non_device_principals_rejected')
        device_id = registered['device_id']
        command('docker', 'exec', 'pab-android-name-test-db', 'psql', '-U', 'postgres', '-d', 'postgres', '-c',
            f"UPDATE devices SET name_changed_at=clock_timestamp()-interval '11 seconds' WHERE id='{uuid.UUID(device_id)}'")
        assert rpc(second, 'rename_own_device', name='Final phone')['type'] == 'own_device_renamed'
        assert rpc(first, 'rename_own_device', name='Final phone')['type'] == 'own_device_renamed'
        row = command('docker', 'exec', 'pab-android-name-test-db', 'psql', '-U', 'postgres', '-d', 'postgres', '-Atc',
            f"SELECT name||':'||revision::text FROM devices WHERE id='{uuid.UUID(device_id)}'")
        assert row == 'Final phone:3', row
        passed('cooldown_expires_without_reauthenticating_existing_connections')
        result['status'] = 'passed'
    finally:
        for ws in sockets:
            ws.close()
        if started:
            subprocess.run(['docker', 'stop', '--timeout', '2', container], stdout=subprocess.DEVNULL)
        (run/'result.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8')
        print(str(run/'result.json'), flush=True)


if __name__ == '__main__':
    main()
