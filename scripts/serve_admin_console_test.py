"""Build and serve an isolated admin-console HTTPS fixture; never installs Pixels.

Requires the explicit test environment TOML used by test_admin_console.py.
The fixture database and logs are retained for diagnosis. Stop with Ctrl+C.
"""
import argparse
from datetime import datetime, timedelta, timezone
import ipaddress
import json
from pathlib import Path
import shutil
import socket
import subprocess
import tomllib
from urllib.parse import urlsplit
import uuid

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID

from service_config import write_config
from test_admin_console import ROOT, build_environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--port', type=int, default=38623)
    parser.add_argument('--binary-dir', type=Path, help='Use already built workspace-local binaries')
    parser.add_argument('--prepare-only', action='store_true', help='Prepare the isolated DB/config without starting the service')
    args = parser.parse_args()
    config = tomllib.loads(args.config.read_text(encoding='utf-8'))
    info = json.loads(subprocess.check_output(['docker', 'inspect', config['container']], text=True))[0]
    assert config.get('isolated') is True
    assert info['Config']['Labels'].get('pixels.task') == 'admin-console-20261020'
    assert info['State']['Running']
    database = urlsplit(config['database_url'])
    assert database.hostname == '127.0.0.1'
    assert any(p['HostIp'] == '127.0.0.1' and int(p['HostPort']) == database.port for p in info['NetworkSettings']['Ports']['5432/tcp'])
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', args.port))
    out = args.config.resolve().parent / 'https'
    out.mkdir(exist_ok=True)
    environment = build_environment(config['database_url'])
    binary_dir=ROOT/'target/debug'
    if args.binary_dir:
        binary_dir=args.binary_dir.resolve()
        binary_dir.relative_to(ROOT)
        assert binary_dir.is_relative_to(ROOT/'.build') or binary_dir.is_relative_to(ROOT/'target')
    else:
        command = ['cargo', 'build', '-p', 'pab-server', '--bin', 'pab-server', '--jobs', '2']
        with (out / 'build.log').open('w', encoding='utf-8') as log:
            subprocess.run(command, cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT, check=True)
        print('Server build complete (2 jobs).', flush=True)
    fixture_database = 'admin_browser_' + uuid.uuid4().hex
    subprocess.run(['docker', 'exec', config['container'], 'psql', '-X', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-c', f'CREATE DATABASE "{fixture_database}"'], check=True, capture_output=True)
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
    now = datetime.now(timezone.utc)
    certificate = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
        .serial_number(x509.random_serial_number()).not_valid_before(now-timedelta(minutes=1))
        .not_valid_after(now+timedelta(days=2)).add_extension(x509.SubjectAlternativeName([
            x509.DNSName('localhost'), x509.IPAddress(ipaddress.ip_address('127.0.0.1'))]), False).sign(key, hashes.SHA256()))
    (out / 'cert.pem').write_bytes(certificate.public_bytes(serialization.Encoding.PEM))
    (out / 'key.pem').write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    server_config = write_config(out / 'pab-server.toml', {
        'listen': f'127.0.0.1:{args.port}', 'database': {'url': f'postgres://postgres@127.0.0.1:{database.port}/{fixture_database}'},
        'tls': {'cert': out / 'cert.pem', 'key': out / 'key.pem'},
        'web': {'assets': ROOT / 'apps/web/dist', 'origin': f'https://127.0.0.1:{args.port}'},
        'relay': {'control_secret': 'isolated-admin-browser-fixture-only-secret'},
        'log': {'directory': out / 'logs'},
    })
    executable = out / 'pab-server.exe'
    shutil.copy2(binary_dir / 'pab-server.exe', executable)
    subprocess.run([str(executable), 'init', '--config', str(server_config)], env=environment, check=True)
    print(f'Isolated admin browser fixture ready: https://127.0.0.1:{args.port}', flush=True)
    if args.prepare_only:
        return
    subprocess.run([str(executable), 'serve', '--config', str(server_config)], env=environment, check=True)


if __name__ == '__main__':
    main()
