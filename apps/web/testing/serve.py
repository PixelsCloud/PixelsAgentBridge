"""Run the fixed isolated HTTPS backend for Web tests (Python cryptography required)."""
from pathlib import Path
from datetime import datetime, timezone, timedelta
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID
import ipaddress
import os
import shutil
import socket
import subprocess

root = Path(__file__).resolve().parents[3]
out = root / '.build/web-test'
out.mkdir(parents=True, exist_ok=True)
with socket.socket() as probe:
    if probe.connect_ex(('127.0.0.1', 38443)) == 0:
        raise SystemExit('Port38443 is in use. Stop your previous isolated test Server first.')
environment = os.environ.copy()
environment.update(CARGO_PROFILE_DEV_DEBUG='0', CARGO_INCREMENTAL='0', CARGO_BUILD_JOBS='8')
subprocess.run(['cargo', 'build', '--locked', '-p', 'pab-server', '--bin', 'pab-server', '--example', 'web-device-fixture'], cwd=root, env=environment, check=True)
key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, 'localhost')])
now = datetime.now(timezone.utc)
cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
        .serial_number(x509.random_serial_number()).not_valid_before(now-timedelta(minutes=1))
        .not_valid_after(now+timedelta(days=7)).add_extension(x509.SubjectAlternativeName([
            x509.DNSName('localhost'), x509.IPAddress(ipaddress.ip_address('127.0.0.1'))]), False)
        .sign(key, hashes.SHA256()))
(out / 'cert.pem').write_bytes(cert.public_bytes(serialization.Encoding.PEM))
(out / 'key.pem').write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
environment.update(PAB_DATABASE_URL='postgres://postgres@127.0.0.1:55435/pab_web_test',
                   PAB_LISTEN_ADDR='127.0.0.1:38443', PAB_TLS_CERT=str(out/'cert.pem'), PAB_TLS_KEY=str(out/'key.pem'),
                   PAB_WEB_DIR=str(root/'apps/web/dist'), PAB_WEB_ORIGIN='https://localhost:38443',
                   PAB_LOG_DIR=str(out/'logs'),
                   PAB_RELAY_CONTROL_SECRET='isolated-web-test-relay-secret-not-a-production-key',
                   PAB_REGISTRATION_ENABLED='true')
# Never inherit a developer's production deployment identity or secret file.
for setting in ['PAB_DEPLOYMENT_ID', 'PAB_RELAY_CONTROL_SECRET_FILE']:
    environment.pop(setting, None)
executable = 'pab-server.exe' if os.name == 'nt' else 'pab-server'
server = out / executable
shutil.copy2(root/'target/debug'/executable, server)
subprocess.run([str(server), 'init'], env=environment, check=True)
print('Isolated Web tests: https://localhost:38443 — leave this terminal running.', flush=True)
try:
    subprocess.run([str(server), 'serve'], env=environment, check=True)
except KeyboardInterrupt:
    pass
