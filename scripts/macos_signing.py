"""Persistent, local-only code signing. Never installs a system trust root.

Run `python3 scripts/macos_signing.py init` once as the build user. Back up the
private state directory securely: replacing its certificate changes TCC identity.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import secrets
import subprocess
import sys
import tempfile

STATE = Path.home() / 'Library/Application Support/PixelsAgentBridgeBuildSigning'
IDENTIFIERS = {'desktop': 'vip.rgaa.pab.desktop', 'executor': 'vip.rgaa.pab.executor',
               'mcp': 'vip.rgaa.pab.mcp'}


def run(args, **kwargs):
    # Never echo argv: security/openssl receive ephemeral keychain passwords.
    result = subprocess.run([str(x) for x in args], capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f'{Path(args[0]).name} failed: {result.stderr.decode(errors="replace")}')
    return result.stdout


def requirement(identifier, fingerprint):
    if identifier not in IDENTIFIERS.values() or not re.fullmatch(r'[0-9A-Fa-f]{40}', fingerprint):
        raise ValueError('Invalid signing identifier or certificate fingerprint')
    # Pin the certificate, not just a freely reproducible bundle identifier.
    return f'identifier "{identifier}" and certificate leaf = H"{fingerprint.upper()}"'


def load_identity(state=STATE):
    if sys.platform != 'darwin':
        raise RuntimeError('macOS signing must run on macOS')
    for name in ('identity.json', 'certificate.der', 'keychain.password', 'signing.keychain-db'):
        p = state / name
        if not p.is_file() or p.is_symlink():
            raise RuntimeError('Signing identity missing/incomplete; run scripts/macos_signing.py init. '
                               'Never delete existing keys to repair a build.')
    for p in (state, state / 'keychain.password', state / 'signing.keychain-db'):
        if p.stat().st_uid != os.getuid() or p.stat().st_mode & 0o077:
            raise RuntimeError(f'Signing state must be private to the build user: {p}')
    fingerprint = hashlib.sha1((state / 'certificate.der').read_bytes()).hexdigest().upper()
    if json.loads((state / 'identity.json').read_text())['sha1'] != fingerprint:
        raise RuntimeError('Signing certificate differs from pinned identity')
    return fingerprint


def initialize(state=STATE):
    if state.exists():
        fingerprint = load_identity(state)
        print(f'Reusing signing certificate {fingerprint}')
        return
    if sys.platform != 'darwin' or os.getuid() == 0:
        raise RuntimeError('Initialize as the macOS build user, not root')
    state.mkdir(parents=True, mode=0o700)
    password = secrets.token_urlsafe(48)
    (state / 'keychain.password').write_text(password)
    (state / 'keychain.password').chmod(0o600)
    keychain = state / 'signing.keychain-db'
    run(['/usr/bin/security', 'create-keychain', '-p', password, keychain])
    keychain.chmod(0o600)
    run(['/usr/bin/security', 'set-keychain-settings', '-lut', '21600', keychain])
    with tempfile.TemporaryDirectory(prefix='identity-', dir=state) as temporary:
        folder = Path(temporary)
        config = folder / 'certificate.conf'
        config.write_text('[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ext\n'
                          '[dn]\nCN=Pixels Agent Bridge Local Code Signing\n'
                          '[ext]\nbasicConstraints=critical,CA:FALSE\n'
                          'keyUsage=critical,digitalSignature\nextendedKeyUsage=critical,codeSigning\n')
        key, cert, p12 = (folder / n for n in ('key.pem', 'certificate.pem', 'identity.p12'))
        run(['/usr/bin/openssl', 'req', '-new', '-x509', '-newkey', 'rsa:3072', '-nodes',
             '-sha256', '-days', '3650', '-config', config, '-keyout', key, '-out', cert])
        key.chmod(0o600)
        run(['/usr/bin/openssl', 'x509', '-in', cert, '-outform', 'DER', '-out', state / 'certificate.der'])
        env = dict(os.environ, PAB_SIGNING_TEMP_PASSWORD=password)
        run(['/usr/bin/openssl', 'pkcs12', '-export', '-inkey', key, '-in', cert,
             '-out', p12, '-passout', 'env:PAB_SIGNING_TEMP_PASSWORD'], env=env)
        run(['/usr/bin/security', 'import', p12, '-k', keychain, '-P', password,
             '-T', '/usr/bin/codesign'])
    fingerprint = hashlib.sha1((state / 'certificate.der').read_bytes()).hexdigest().upper()
    (state / 'identity.json').write_text(json.dumps({'sha1': fingerprint, 'kind': 'self-signed'}, indent=2) + '\n')
    (state / 'identity.json').chmod(0o600)
    print(f'Created local signing certificate {load_identity(state)} (not notarized)')


def sign(path, identifier, state=STATE):
    fingerprint = load_identity(state)
    keychain = state / 'signing.keychain-db'
    run(['/usr/bin/security', 'unlock-keychain', '-p', (state / 'keychain.password').read_text(), keychain])
    run(['/usr/bin/codesign', '--force', '--sign', fingerprint, '--keychain', keychain,
         '--timestamp=none', '--options', 'runtime', '--identifier', identifier,
         '--requirements', '=designated => ' + requirement(identifier, fingerprint), path])
    verify(path, identifier, fingerprint)


def verify(path, identifier, fingerprint):
    run(['/usr/bin/codesign', '--verify', '--strict', '--deep',
         '--test-requirement', '=' + requirement(identifier, fingerprint), path])
    result = subprocess.run(['/usr/bin/codesign', '-d', '-r-', str(path)], capture_output=True, text=True, check=True)
    dr = result.stdout + result.stderr
    lines = [line.removeprefix('designated => ') for line in dr.splitlines() if line.startswith('designated => ')]
    actual = lines[0].strip().replace('"', '').lower() if len(lines) == 1 else ''
    if actual != requirement(identifier, fingerprint).replace('"', '').lower():
        raise RuntimeError('Designated requirement is not the pinned, version-independent identity')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['init', 'check'])
    args = parser.parse_args()
    if args.action == 'init':
        initialize()
    else:
        print(load_identity())
