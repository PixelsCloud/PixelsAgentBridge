"""Build and publish a versioned desktop installer through the site release API."""

from argparse import ArgumentParser
from base64 import b64encode
from hashlib import sha256
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.parse import urlsplit
from urllib.request import Request, urlopen
import http.client
import json
import os
import subprocess
import sys
import uuid

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = ROOT / '.build/packages'


def request_json(base, route, *, token=None, body=None):
    headers = {'Accept': 'application/json'}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    if body is not None:
        headers['Content-Type'] = 'application/json'
    data = None if body is None else json.dumps(body, separators=(',', ':')).encode()
    request = Request(base + route, data=data, headers=headers, method='POST' if body is not None else 'GET')
    try:
        with urlopen(request, timeout=40) as response:
            return None if response.status == 204 else json.load(response)
    except HTTPError as error:
        try:
            details = json.load(error)['error']
        except (ValueError, KeyError):
            details = 'HTTP ' + str(error.code)
        raise RuntimeError(f'Release API {route}: {details}') from None
    except URLError:
        raise RuntimeError(f'Release API {route}: network unavailable') from None


def digest(path):
    value = sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def write_state(path, state):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(state, indent=2) + '\n', encoding='utf-8')
    os.replace(temporary, path)


def canonical(platform, arch, version, filename, size, hash_value):
    return f'PAB-RELEASE-V1\n{platform}\n{arch}\nstable\n{version}\n{filename}\n{size}\n{hash_value}\n'.encode()


def package_info(platform, arch, version):
    if platform == 'linux-desktop':
        from importlib.util import spec_from_file_location, module_from_spec
        directory = ROOT / '.build/linux-desktop' / version
        manifest = json.loads((directory / 'package.json').read_text(encoding='utf-8'))
        filename = f'pixels-agent-bridge-desktop-linux-{arch}-{version}.deb'
        path = directory / filename
        size, hash_value = path.stat().st_size, digest(path)
        if (manifest.get('version'), manifest.get('platform'), manifest.get('architecture'), manifest.get('filename'), manifest.get('bytes'), manifest.get('sha256')) != (version, platform, arch, filename, size, hash_value):
            raise RuntimeError('Linux desktop package does not match its build receipt')
        spec = spec_from_file_location('linux_desktop_package', ROOT / 'packaging/desktop/build_linux_desktop_package.py')
        packager = module_from_spec(spec)
        spec.loader.exec_module(packager)
        packager.verify_desktop(directory / 'pab-desktop', version)
        if manifest.get('desktop_sha256') != digest(directory / 'pab-desktop'):
            raise RuntimeError('Linux desktop package refers to a different GUI executable')
        return path, size, hash_value
    if platform == 'linux':
        filename = f'pixels-agent-bridge-linux-{arch}-release-{version}.tar.gz'
        path = PACKAGES / filename
        manifest = PACKAGES / 'SHA256.json'
        if not path.is_file() or not manifest.is_file():
            raise RuntimeError('Build did not produce the expected Linux package and checksum manifest')
        metadata = json.loads(manifest.read_text(encoding='utf-8')).get(filename, {})
        size, hash_value = path.stat().st_size, digest(path)
        if (metadata.get('version'), metadata.get('bytes'), metadata.get('sha256')) != (version, size, hash_value):
            raise RuntimeError('Linux package does not match its build checksum manifest')
        return path, size, hash_value
    extension = 'exe' if platform == 'windows' else 'pkg'
    filename = f'pixels-agent-bridge-{platform}-{arch}-release-{version}-setup.{extension}'
    manifest = PACKAGES / (f'SHA256-windows-setup-release.json' if platform == 'windows' else f'SHA256-macos-{arch}-setup-release.json')
    path = PACKAGES / filename
    if not path.is_file() or not manifest.is_file():
        raise RuntimeError('Build did not produce the expected installer and checksum manifest')
    metadata = json.loads(manifest.read_text(encoding='utf-8'))
    expected = (filename, version, path.stat().st_size, digest(path))
    actual = (metadata.get('file'), metadata.get('version'), metadata.get('bytes'), metadata.get('sha256'))
    if actual != expected:
        raise RuntimeError('Installer does not match its build checksum manifest')
    return path, expected[2], expected[3]


def notarize_mac(arch, version, profile):
    path = PACKAGES / f'pixels-agent-bridge-macos-{arch}-release-{version}-setup.pkg'
    manifest = PACKAGES / f'SHA256-macos-{arch}-setup-release.json'
    subprocess.run(['xcrun', 'notarytool', 'submit', str(path), '--keychain-profile', profile, '--wait'], check=True)
    subprocess.run(['xcrun', 'stapler', 'staple', str(path)], check=True)
    subprocess.run(['xcrun', 'stapler', 'validate', str(path)], check=True)
    metadata = json.loads(manifest.read_text(encoding='utf-8'))
    metadata.update(bytes=path.stat().st_size, sha256=digest(path), notarized=True)
    manifest.write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')


def upload(url, path):
    # Stream the file. Never print the signed URL or let a library exception expose it.
    parsed = urlsplit(url)
    if parsed.scheme != 'https' or not parsed.hostname or not parsed.query:
        raise RuntimeError('Invalid temporary COS upload URL')
    connection = http.client.HTTPSConnection(parsed.hostname, parsed.port or 443, timeout=900)
    try:
        connection.putrequest('PUT', parsed.path + '?' + parsed.query)
        connection.putheader('Content-Length', str(path.stat().st_size))
        connection.putheader('Content-Type', 'application/octet-stream')
        connection.endheaders()
        with path.open('rb') as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b''):
                connection.send(chunk)
        response = connection.getresponse()
        if response.status not in (200, 201, 204):
            raise RuntimeError(f'COS upload failed (HTTP {response.status}); rerun with --resume')
        response.read()
    except OSError:
        raise RuntimeError('COS upload failed; rerun with --resume') from None
    finally:
        connection.close()


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('platform', choices=('windows', 'macos', 'linux', 'linux-desktop'))
    parser.add_argument('--arch', default=None, choices=('x86_64', 'aarch64'))
    parser.add_argument('--site', default='https://agent.rgaa.vip')
    parser.add_argument('--notes-zh', required=True)
    parser.add_argument('--notes-en', required=True)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--allow-dirty', action='store_true', help='For release rehearsals only')
    parser.add_argument('--prepare-only', action='store_true', help='Build a candidate without uploading; run smoke before --resume')
    parser.add_argument('--smoke-report', type=Path, help='Matching smoke/runs/<id>/report.json required before publication')
    parser.add_argument('--headless-package', type=Path, help='Verified backend archive for the Linux desktop build')
    parser.add_argument('--headless-sha256', help='Expected SHA-256 of that archive')
    args = parser.parse_args()
    arch = args.arch or ('aarch64' if args.platform == 'macos' else 'x86_64')
    if (args.platform, arch) not in (('windows', 'x86_64'), ('macos', 'aarch64'), ('macos', 'x86_64'), ('linux', 'x86_64'), ('linux-desktop', 'x86_64')):
        parser.error('Stable supports Windows/Linux x86_64 and macOS aarch64/x86_64')
    if args.platform == 'linux-desktop' and not (args.headless_package and args.headless_sha256):
        parser.error('Linux desktop requires --headless-package and --headless-sha256')
    base = args.site.rstrip('/')
    if not base.startswith('https://'):
        parser.error('--site must be HTTPS')
    token = os.environ.get('PAB_RELEASE_API_TOKEN')
    key_path = os.environ.get('PAB_RELEASE_PRIVATE_KEY_FILE')
    if not token or not key_path:
        parser.error('Set PAB_RELEASE_API_TOKEN and PAB_RELEASE_PRIVATE_KEY_FILE privately')
    private = Ed25519PrivateKey.from_private_bytes(Path(key_path).read_bytes())
    installer_identity = os.environ.get('PAB_MACOS_INSTALLER_IDENTITY') if args.platform == 'macos' else None
    notary_profile = os.environ.get('PAB_MACOS_NOTARY_PROFILE') if args.platform == 'macos' else None
    if notary_profile and not installer_identity:
        parser.error('PAB_MACOS_NOTARY_PROFILE requires PAB_MACOS_INSTALLER_IDENTITY')
    commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    if not args.allow_dirty and subprocess.check_output(
        ['git', 'status', '--porcelain', '--untracked-files=no', '--', '.', ':!build-version.json'], cwd=ROOT
    ).strip():
        parser.error('Commit tracked changes before publishing, or use --allow-dirty for a rehearsal')
    state_path = ROOT / '.build' / f'release-{args.platform}-{arch}.json'
    if args.resume:
        if not state_path.is_file():
            parser.error('No release state to resume')
        state = json.loads(state_path.read_text(encoding='utf-8'))
        if state['platform'] != args.platform or state['arch'] != arch:
            parser.error('Saved release target does not match')
    else:
        if state_path.exists():
            parser.error('An unfinished release exists; use --resume or resolve its state first')
        build_id = str(uuid.uuid4())
        state = {'platform': args.platform, 'arch': arch, 'commit': commit, 'build_id': build_id, 'phase': 'requesting'}
        write_state(state_path, state)
    if state['phase'] == 'requesting':
        reserved = request_json(base, '/api/admin/release-reservations', token=token,
            body={'platform': args.platform, 'architecture': arch, 'channel': 'stable', 'build_id': state['build_id'], 'commit': state['commit']})
        state.update(reservation_id=reserved['reservation_id'], version=reserved['version'], phase='reserved')
        write_state(state_path, state)
    version = state['version']
    if state['phase'] == 'reserved':
        target = 'desktop' if args.platform == 'windows' else args.platform
        command = ([sys.executable, 'scripts/build_linux_desktop.py', '--version', version, '--package',
                    '--headless-package', str(args.headless_package.resolve()), '--headless-sha256', args.headless_sha256]
                   if args.platform == 'linux-desktop' else
                   [sys.executable, 'scripts/build.py', target, '--profile', 'release', '--package', '--installer-version', version])
        if args.platform == 'macos':
            command += ['--macos-arch', arch]
            if installer_identity:
                command += ['--macos-installer-identity', installer_identity]
        subprocess.run(command, cwd=ROOT, check=True)
        if args.platform == 'macos' and notary_profile:
            notarize_mac(arch, version, notary_profile)
        state['phase'] = 'built'
        write_state(state_path, state)
    path, size, hash_value = package_info(args.platform, arch, version)
    if args.prepare_only:
        if state['phase'] != 'built':
            raise RuntimeError('--prepare-only requires a built, not uploaded candidate')
        print(f'Candidate ready: {path}. Run smoke/run.py, then --resume --smoke-report REPORT.')
        return
    if state['phase'] != 'published':
        if not args.smoke_report:
            raise RuntimeError('Publication blocked: run release smoke for this candidate and resume with --smoke-report')
        sys.path.insert(0, str(ROOT))
        from smoke.run import validate_release_report
        validate_release_report(args.smoke_report, platform=args.platform, arch=arch, artifact=path,
                                version=version, commit=state['commit'])
    signature = b64encode(private.sign(canonical(args.platform, arch, version, path.name, size, hash_value))).decode()
    if state['phase'] == 'built':
        created = request_json(base, '/api/admin/upload-sessions', token=token, body={
            'reservation_id': state['reservation_id'], 'filename': path.name, 'size': size,
            'sha256': hash_value, 'signature': signature, 'notes_zh': args.notes_zh,
            'notes_en': args.notes_en, 'idempotency_key': state['build_id']})
        state['upload_id'] = created['upload_id']
        state['phase'] = 'uploading'
        write_state(state_path, state)
        upload_url = created['put_url']
    else:
        upload_url = None
    if state['phase'] == 'uploading':
        if upload_url is None:
            current = request_json(base, '/api/admin/upload-sessions/' + state['upload_id'], token=token)
            upload_url = current['put_url']
            if current['state'] == 'published':
                state['phase'] = 'published'
        if upload_url:
            upload(upload_url, path)
            state['phase'] = 'uploaded'
            write_state(state_path, state)
    if state['phase'] == 'uploaded':
        request_json(base, '/api/admin/upload-sessions/' + state['upload_id'] + '/finalize', token=token, body={})
        state['phase'] = 'published'
        write_state(state_path, state)
    info = request_json(base, '/api/updates/check?' + urlencode({
        'platform': args.platform, 'architecture': arch, 'channel': 'stable', 'current': '0.0.0'}))
    if not info or info['version'] != version or info['sha256'] != hash_value:
        raise RuntimeError('Published, but public update API verification failed')
    state_path.unlink()
    print(f'Published {args.platform} {arch} {version}: {info["download_url"]}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Release failed: {error}', file=sys.stderr)
        sys.exit(1)
