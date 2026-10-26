"""Persistent, local-only code signing. Never installs a system trust root.

Run `python3 scripts/macos_signing.py init` once as the build user. Back up the
private state directory securely: replacing its certificate changes TCC identity.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import plistlib
import re
import secrets
import shlex
import shutil
import subprocess
import sys
import tempfile

STATE = Path(os.environ.get('PAB_MACOS_SIGNING_STATE_DIR',
                            Path.home() / 'Library/Application Support/PixelsAgentBridgeBuildSigning'))
IDENTIFIERS = {'desktop': 'vip.rgaa.pab.desktop', 'executor': 'vip.rgaa.pab.executor',
               'mcp': 'vip.rgaa.pab.mcp', 'document': 'vip.rgaa.pab.document',
               'pdfium': 'vip.rgaa.pab.pdfium'}


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


def import_ci_identity(state=STATE):
    """Import the existing pinned identity into a fresh hosted runner, never generate one."""
    import base64
    from cryptography.hazmat.primitives.serialization import Encoding, pkcs12
    if (sys.platform != 'darwin' or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted'):
        raise RuntimeError('CI identity import is only available on a macOS Actions runner')
    if state.exists():
        raise RuntimeError('Refusing to replace an existing signing state')
    try:
        encoded = base64.b64decode(os.environ['PAB_MACOS_SIGNING_P12_BASE64'], validate=True)
        password = os.environ['PAB_MACOS_SIGNING_P12_PASSWORD']
        key, certificate, _ = pkcs12.load_key_and_certificates(encoded, password.encode())
        if key is None or certificate is None:
            raise ValueError()
        der = certificate.public_bytes(Encoding.DER)
        fingerprint = hashlib.sha1(der).hexdigest().upper()
        if fingerprint != os.environ['PAB_MACOS_CERT_SHA1'].replace(':', '').upper():
            raise ValueError()
    except (KeyError, ValueError, TypeError):
        raise RuntimeError('Existing macOS signing material or pinned certificate is invalid') from None
    state.mkdir(parents=True, mode=0o700)
    keychain_password = secrets.token_urlsafe(48)
    (state / 'keychain.password').write_text(keychain_password)
    (state / 'keychain.password').chmod(0o600)
    keychain = state / 'signing.keychain-db'
    run(['/usr/bin/security', 'create-keychain', '-p', keychain_password, keychain])
    keychain.chmod(0o600)
    run(['/usr/bin/security', 'set-keychain-settings', '-lut', '21600', keychain])
    run(['/usr/bin/security', 'unlock-keychain', '-p', keychain_password, keychain])
    with tempfile.TemporaryDirectory(prefix='import-', dir=state) as temporary:
        p12 = Path(temporary) / 'identity.p12'
        p12.write_bytes(encoded)
        p12.chmod(0o600)
        run(['/usr/bin/security', 'import', p12, '-k', keychain, '-P', password, '-T', '/usr/bin/codesign'])
        run(['/usr/bin/security', 'set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:',
             '-s', '-k', keychain_password, keychain])
    # codesign's private-key lookup also uses the user search list. Preserve the
    # runner's existing keychains rather than replacing the list with this one.
    search = shlex.split(run(['/usr/bin/security', 'list-keychains', '-d', 'user']).decode())
    if str(keychain) not in search:
        run(['/usr/bin/security', 'list-keychains', '-d', 'user', '-s', *search, keychain])
    (state / 'certificate.der').write_bytes(der)
    (state / 'identity.json').write_text(json.dumps({'sha1': fingerprint, 'kind': 'imported'}) + '\n')
    (state / 'identity.json').chmod(0o600)
    load_identity(state)
    # Detect unusable imports before spending time on a complete native build.
    with tempfile.TemporaryDirectory(prefix='signing-probe-', dir=state) as temporary:
        probe = Path(temporary) / 'probe'
        shutil.copyfile('/usr/bin/true', probe)
        probe.chmod(0o755)
        sign(probe, IDENTIFIERS['executor'], state)
    print(f'Imported existing identity {fingerprint}; signing and verification passed')


def sign(path, identifier, state=STATE):
    fingerprint = load_identity(state)
    keychain = state / 'signing.keychain-db'
    run(['/usr/bin/security', 'unlock-keychain', '-p', (state / 'keychain.password').read_text(), keychain])
    command = ['/usr/bin/codesign', '--force', '--sign', fingerprint, '--keychain', keychain,
               '--timestamp=none', '--options', 'runtime', '--identifier', identifier,
               '--requirements', '=designated => ' + requirement(identifier, fingerprint)]
    if identifier == IDENTIFIERS['document']:
        # A local self-signed identity has no Apple Team ID. Hardened Runtime
        # would reject the bundled PDFium dylib even though both files use the
        # same certificate. Scope this exception to the disposable parser.
        with tempfile.TemporaryDirectory(prefix='pab-document-sign-') as folder:
            entitlements = Path(folder) / 'entitlements.plist'
            entitlements.write_bytes(plistlib.dumps({
                'com.apple.security.cs.disable-library-validation': True,
            }))
            run([*command, '--entitlements', entitlements, path])
    else:
        run([*command, path])
    verify(path, identifier, fingerprint)


def verify(path, identifier, fingerprint):
    run(['/usr/bin/codesign', '--verify', '--strict', '--deep',
         '--test-requirement', '=' + requirement(identifier, fingerprint), path])
    if identifier == IDENTIFIERS['document']:
        entitlements = run(['/usr/bin/codesign', '-d', '--entitlements', '-', path])
        if b'com.apple.security.cs.disable-library-validation' not in entitlements or b'true' not in entitlements:
            raise RuntimeError('Document worker lacks the PDFium library-validation entitlement')
    result = subprocess.run(['/usr/bin/codesign', '-d', '-r-', str(path)], capture_output=True, text=True, check=True)
    dr = result.stdout + result.stderr
    lines = [line.removeprefix('designated => ') for line in dr.splitlines() if line.startswith('designated => ')]
    actual = lines[0].strip().replace('"', '').lower() if len(lines) == 1 else ''
    if actual != requirement(identifier, fingerprint).replace('"', '').lower():
        raise RuntimeError('Designated requirement is not the pinned, version-independent identity')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['init', 'check', 'import-ci'])
    args = parser.parse_args()
    if args.action == 'import-ci':
        import_ci_identity()
    elif args.action == 'init':
        initialize()
    else:
        print(load_identity())
