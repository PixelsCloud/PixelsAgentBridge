#!/usr/bin/env bash
# Published verbatim at https://agent.rgaa.vip/install.sh.
# Keep execution inside main so a truncated download cannot start installation.
main() {
    set -euo pipefail
    if [[ ${1:-} == --help || ${1:-} == -h ]]; then
        printf '%s\n' 'Pixels Linux installer' 'Usage: bash install.sh [--no-service] [--control-url wss://HOST/control --relay-url https://HOST]'
        return
    fi
    [[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || {
        echo 'Pixels currently supports Linux x86_64 only.' >&2; return 1;
    }
    local -a privilege=()
    if [[ $EUID -ne 0 ]]; then
        command -v sudo >/dev/null || { echo 'Run as root or install sudo.' >&2; return 1; }
        privilege=(sudo)
    fi
    # Python parses the signed metadata and extracts only regular package files.
    if ! command -v python3 >/dev/null || ! command -v openssl >/dev/null; then
        echo 'Installing Python 3, OpenSSL and CA certificates...'
        if command -v apt-get >/dev/null; then
            "${privilege[@]}" apt-get update
            "${privilege[@]}" apt-get install -y python3 openssl ca-certificates
        elif command -v dnf >/dev/null; then
            "${privilege[@]}" dnf install -y python3 openssl ca-certificates
        else
            echo 'Install python3, openssl and ca-certificates, then retry.' >&2; return 1
        fi
    fi
    python3 - "$@" <<'PY'
import argparse
import base64
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

SITE = 'https://agent.rgaa.vip'
PUBLIC_KEY = 'dOqSsbkPvJmGFblm/tH23cwbRmHjVhu6POF+WLmnKXk='
REQUIRED = {'pab-executor', 'pab-mcp', 'pab-document-worker',
            'document-runtime-linux.tar.gz', 'install.sh', 'uninstall.sh',
            'lifecycle.sh', 'run-mcp.sh', 'run-executor.sh', 'INSTALL-LINUX.txt', 'release.json'}


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise RuntimeError('Unexpected download redirect; installation stopped.')


def fetch(url):
    if urlsplit(url).scheme != 'https':
        raise RuntimeError('Downloads require HTTPS.')
    try:
        return build_opener(NoRedirect).open(Request(url, headers={'User-Agent': 'Pixels-Linux-Installer'}), timeout=90)
    except (HTTPError, URLError) as error:
        # Do not expose credentials or URL query strings through exception text.
        raise RuntimeError('Download unavailable. Check the network and retry.') from None


def validate_manifest(info, directory, public_key=PUBLIC_KEY):
    if (info.get('schema'), info.get('platform'), info.get('architecture'), info.get('channel')) != (1, 'linux', 'x86_64', 'stable'):
        raise RuntimeError('Release does not match Linux x86_64 stable.')
    version = info.get('version', '')
    if not isinstance(version, str) or not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)', version):
        raise RuntimeError('Invalid release version.')
    filename = f'pixels-agent-bridge-linux-x86_64-release-{version}.tar.gz'
    if info.get('filename') != filename or type(info.get('size')) is not int or not 0 < info['size'] <= 5_000_000_000:
        raise RuntimeError('Invalid package name or size.')
    if not isinstance(info.get('sha256'), str) or not re.fullmatch('[0-9a-f]{64}', info['sha256']):
        raise RuntimeError('Invalid package checksum.')
    if not isinstance(info.get('download_url'), str):
        raise RuntimeError('Invalid package download address.')
    url = urlsplit(info['download_url'])
    expected_path = f'/releases/linux/x86_64/{version}/{filename}'
    if (url.scheme != 'https' or not (url.hostname or '').endswith('.cos.ap-beijing.myqcloud.com')
            or url.path != expected_path or url.username or url.password or url.query or url.fragment or url.port not in (None, 443)):
        raise RuntimeError('Untrusted package download address.')
    canonical = f'PAB-RELEASE-V1\nlinux\nx86_64\nstable\n{version}\n{filename}\n{info["size"]}\n{info["sha256"]}\n'
    try:
        signature = base64.b64decode(info['signature'], validate=True)
        key = base64.b64decode(public_key, validate=True)
    except (KeyError, ValueError, TypeError):
        raise RuntimeError('Invalid release signature.') from None
    if len(signature) != 64 or len(key) != 32:
        raise RuntimeError('Invalid release signature.')
    (directory / 'manifest').write_bytes(canonical.encode())
    (directory / 'signature').write_bytes(signature)
    # RFC 8410 SubjectPublicKeyInfo for Ed25519, followed by the raw public key.
    (directory / 'public.der').write_bytes(bytes.fromhex('302a300506032b6570032100') + key)
    result = subprocess.run(['openssl', 'pkeyutl', '-verify', '-pubin', '-keyform', 'DER',
        '-inkey', str(directory / 'public.der'), '-rawin', '-in', str(directory / 'manifest'),
        '-sigfile', str(directory / 'signature')], capture_output=True)
    if result.returncode:
        raise RuntimeError('Release signature verification failed. OpenSSL 3 or newer is required.')


def download_package(info, path):
    total = 0
    digest = hashlib.sha256()
    with fetch(info['download_url']) as response, path.open('wb') as output:
        while True:
            chunk = response.read(1024 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > info['size']:
                raise RuntimeError('Package exceeds its signed size.')
            digest.update(chunk)
            output.write(chunk)
    if total != info['size'] or digest.hexdigest() != info['sha256']:
        raise RuntimeError('Package checksum or size mismatch; existing installation was not changed.')


def extract_regular(archive, destination, required=None):
    with tarfile.open(archive, 'r:gz') as package:
        members = package.getmembers()
        names = [member.name for member in members]
        if len(names) > 1000 or len(names) != len(set(names)) or sum(member.size for member in members) > 2_000_000_000:
            raise RuntimeError('Invalid package contents.')
        if required is not None and set(names) != required:
            raise RuntimeError('Package files are incomplete or unexpected.')
        for member in members:
            name = PurePosixPath(member.name)
            if (not member.isfile() or name.is_absolute() or '..' in name.parts
                    or '\\' in member.name or (required is None and not member.name.startswith('document-runtime/'))):
                raise RuntimeError('Unsafe archive entry.')
        for member in members:
            target = destination / member.name
            target.parent.mkdir(parents=True, exist_ok=True)
            with package.extractfile(member) as source, target.open('wb') as output:
                shutil.copyfileobj(source, output)
            target.chmod(0o755 if member.name.startswith('pab-') or member.name.endswith('.sh') else 0o644)


def preflight(payload, info):
    release = json.loads((payload / 'release.json').read_text())
    if (release.get('platform'), release.get('architecture'), release.get('version')) != ('linux', 'x86_64', info['version']):
        raise RuntimeError('Package release metadata does not match its signed manifest.')
    for name in ('pab-executor', 'pab-mcp', 'pab-document-worker', 'document-runtime/libpdfium.so'):
        path = payload / name
        with path.open('rb') as binary:
            header = binary.read(20)
        if header[:6] != b'\x7fELF\x02\x01' or header[18:20] != b'\x3e\x00':
            raise RuntimeError(f'{name} is not an x86_64 Linux executable/library.')
        result = subprocess.run(['ldd', str(path)], capture_output=True, text=True)
        if result.returncode or 'not found' in result.stdout + result.stderr:
            raise RuntimeError(f'{name} requires missing runtime libraries. Install libc6, libgcc and libstdc++ for this distribution and retry.')


def main():
    parser = argparse.ArgumentParser(description='Install or upgrade Pixels Linux headless service.')
    parser.add_argument('--no-service', action='store_true')
    parser.add_argument('--control-url')
    parser.add_argument('--relay-url')
    args = parser.parse_args()
    if bool(args.control_url) != bool(args.relay_url):
        parser.error('Provide both --control-url and --relay-url.')
    if args.control_url and (not args.control_url.startswith('wss://') or not args.relay_url.startswith('https://')):
        parser.error('Control and Relay URLs must use wss:// and https://.')
    if platform.system() != 'Linux' or platform.machine() != 'x86_64':
        raise RuntimeError('Only Linux x86_64 is supported.')
    if not args.no_service and not Path('/run/systemd/system').is_dir():
        raise RuntimeError('systemd is not running. Use --no-service with your process supervisor.')
    for tool in ('openssl', 'tar', 'ldd', 'bash'):
        if not shutil.which(tool):
            raise RuntimeError(f'Required program is missing: {tool}')
    privilege = [] if os.geteuid() == 0 else ['sudo']
    print('Checking the latest signed Linux release...', flush=True)
    with fetch(SITE + '/api/updates/check?platform=linux&architecture=x86_64&channel=stable&current=0.0.0') as response:
        if response.status == 204:
            raise RuntimeError('No Linux release has been published yet. Existing installation was not changed.')
        data = response.read(65537)
        if len(data) > 65536:
            raise RuntimeError('Release metadata is too large.')
        info = json.loads(data)
    if not isinstance(info, dict):
        raise RuntimeError('Invalid release metadata.')
    with tempfile.TemporaryDirectory(prefix='pixels-install-') as temp:
        directory = Path(temp)
        validate_manifest(info, directory)
        print(f'Downloading Pixels {info["version"]} ({info["size"] / 1024 / 1024:.1f} MiB)...', flush=True)
        archive = directory / 'package.tar.gz'
        download_package(info, archive)
        payload = directory / 'payload'
        payload.mkdir()
        extract_regular(archive, payload, REQUIRED)
        extract_regular(payload / 'document-runtime-linux.tar.gz', payload)
        preflight(payload, info)
        command = privilege + ['bash', str(payload / 'install.sh')]
        if args.no_service:
            command.append('--no-service')
        if args.control_url:
            command += [args.control_url, args.relay_url]
        print('Package verified. Installing service...', flush=True)
        subprocess.run(command, check=True, stdin=subprocess.DEVNULL)
    print(f'Pixels {info["version"]} installed. Add this device in Pixels Desktop.')
    print('Show device code/password: sudo /opt/pixels-agent-bridge/run-executor.sh show-access')
    print('Configuration: /etc/pixels-agent-bridge/config.toml (preserved on upgrade)')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError, tarfile.TarError) as error:
        print(f'Pixels installation failed: {error}', file=sys.stderr)
        sys.exit(1)
PY
}
main "$@"
