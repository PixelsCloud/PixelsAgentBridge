"""Build immutable multi-platform GitHub candidates; publish only after full smoke.

Project policy is TOML. JSON files below are runtime manifests/GitHub contracts.
No command installs an application, deploys a server, allocates a version or creates a tag.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from build_version import parse_version
from smoke.run import digest, source_state, validate_release_report

DOMAIN = b'PAB-GITHUB-RELEASE-V1\n'
CONFIG = ROOT / 'packaging/github-release.toml'


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


def configuration():
    value = tomllib.loads(CONFIG.read_text(encoding='utf-8'))
    if value['schema_version'] != 1:
        raise ValueError('Unsupported release policy')
    return value


def plan(tag):
    match = re.fullmatch(r'(client|management|relay)-v([0-9]+\.[0-9]+\.[0-9]+)', tag)
    if not match:
        raise ValueError('Use client-vX.Y.Z, management-vX.Y.Z or relay-vX.Y.Z')
    product, version = match.groups()
    parse_version(version)
    policy = configuration()
    targets = policy['products'][product]['targets']
    return {'tag': tag, 'product': product, 'version': version,
            'latest': policy['products'][product]['latest'],
            'matrix': {'include': [dict(id=target, **policy['targets'][target]) for target in targets]}}


def preflight(tag):
    value = plan(tag)
    names = (['PAB_RELEASE_PRIVATE_KEY_BASE64', 'PAB_RELEASE_PUBLIC_KEY_BASE64',
              'PAB_CI_CLIENT_DEFAULTS_TOML', 'PAB_CI_RELAY_CA_PEM',
              'PAB_MACOS_SIGNING_P12_BASE64', 'PAB_MACOS_SIGNING_P12_PASSWORD', 'PAB_MACOS_CERT_SHA1',
              'PAB_ANDROID_KEYSTORE_BASE64', 'PAB_ANDROID_KEYSTORE_PASSWORD', 'PAB_ANDROID_KEY_ALIAS',
              'PAB_ANDROID_KEY_PASSWORD', 'PAB_ANDROID_CERT_SHA256'] if value['product'] == 'client' else
             ['PAB_SERVICE_RELEASE_PRIVATE_KEY_BASE64', 'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64'])
    missing = [name for name in names if not os.environ.get(name)]
    if missing:
        raise ValueError('Configure release Secrets/Variables before building: ' + ', '.join(missing))
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
    key = signing_key(value['product'])
    public_name = 'PAB_RELEASE_PUBLIC_KEY_BASE64' if value['product'] == 'client' else 'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64'
    if base64.b64decode(os.environ[public_name], validate=True) != key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw):
        raise ValueError('Release signing key does not match the separately pinned public key')


def checked_source(tag):
    value = source_state()
    if value['dirty']:
        raise ValueError('Release candidates require a clean committed checkout')
    if os.environ.get('GITHUB_ACTIONS') == 'true':
        # Both manually selected refs and pushed tags must belong to reviewed main history.
        subprocess.run(['git', 'merge-base', '--is-ancestor', value['commit'], 'origin/main'],
                       cwd=ROOT, check=True, capture_output=True)
        selected = subprocess.check_output(['git', 'rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}'],
                                           cwd=ROOT, text=True).strip()
        if selected != value['commit']:
            raise ValueError('Checked out commit does not match the release tag')
        if os.environ.get('GITHUB_WORKFLOW') == 'Release candidates' and os.environ.get('GITHUB_SHA') != selected:
            raise ValueError('Manual candidate workflows must run with the release tag selected as their Git ref')
    return value


def private_file(name, data):
    folder = ROOT / '.build/github-release/private'
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = folder / name
    if path.exists():
        raise ValueError('Private CI staging already exists; use a fresh runner')
    path.write_bytes(data)
    path.chmod(0o600)
    return path


def build_environment(target):
    env = dict(os.environ)
    env.update(CARGO_BUILD_JOBS='4', CMAKE_BUILD_PARALLEL_LEVEL='1', CARGO_INCREMENTAL='0')
    if env.get('PAB_CI_CLIENT_DEFAULTS_TOML'):
        from service_config import write_config
        defaults = tomllib.loads(env['PAB_CI_CLIENT_DEFAULTS_TOML'])
        if set(defaults) - {'control_url', 'relay_urls'}:
            raise ValueError('CI defaults accept only control_url and relay_urls; supply the public CA separately')
        pem = env.get('PAB_CI_RELAY_CA_PEM', '')
        if pem:
            if 'PRIVATE KEY' in pem or '-----BEGIN CERTIFICATE-----' not in pem:
                raise ValueError('Relay trust must contain public certificates only')
            private_file('relay-ca.pem', pem.encode())
            defaults['relay_ca_cert'] = 'relay-ca.pem'
        path = private_file('client-defaults.toml', b'')
        write_config(path, defaults)
        env['PAB_CLIENT_DEFAULTS_FILE'] = str(path)
    elif env.get('PAB_CI_RELAY_CA_PEM'):
        raise ValueError('Public relay trust requires matching client defaults')
    if target == 'android':
        try:
            key = base64.b64decode(env['PAB_ANDROID_KEYSTORE_BASE64'], validate=True)
        except (KeyError, ValueError):
            raise ValueError('Existing Android release keystore is required') from None
        env['PAB_ANDROID_KEYSTORE_FILE'] = str(private_file('android.keystore', key))
    if target.startswith('macos-'):
        env['PAB_MACOS_SIGNING_STATE_DIR'] = str(ROOT / '.build/github-release/private/macos-signing')
        subprocess.run([sys.executable, 'scripts/macos_signing.py', 'import-ci'], cwd=ROOT, env=env, check=True)
    return env


def build(tag, target):
    value = plan(tag)
    selected = next((t for t in value['matrix']['include'] if t['id'] == target), None)
    if selected is None:
        raise ValueError('Target does not belong to this release')
    before = checked_source(tag)
    product, version = value['product'], value['version']
    env = build_environment(target)
    command = [sys.executable, 'scripts/build.py']
    assets = []

    def run(args):
        subprocess.run([str(a) for a in args], cwd=ROOT, env=env, check=True)

    def asset(path, platform, architecture, name=None):
        path = Path(path).resolve(strict=True)
        assets.append((path, {'name': name or path.name, 'platform': platform,
                             'architecture': architecture, 'sha256': digest(path), 'bytes': path.stat().st_size}))

    if product in ('management', 'relay'):
        run([*command, product, '--profile', 'release', '--package', '--release-version', version])
        from release_service import read_candidate
        extension = '.zip' if selected['platform'] == 'windows' else '.tar.gz'
        path = ROOT / f'.build/packages/pixels-agent-bridge-{product}-{selected["platform"]}-x86_64-release-{version}{extension}'
        read_candidate(path, product)
        asset(path, product, selected['architecture'])
    elif target == 'android':
        run([sys.executable, 'scripts/build_android.py', 'release', '--version', version, '--jobs', '4'])
        asset(ROOT / 'apps/android/app/build/outputs/apk/release/app-release.apk', 'android', 'aarch64',
              f'pixels-agent-bridge-android-universal-release-{version}.apk')
    else:
        from release import package_info
        platform, arch = selected['platform'], selected['architecture']
        build_target = {'windows': 'desktop', 'macos': 'macos', 'linux': 'linux'}[platform]
        extra = ['--macos-arch', arch] if platform == 'macos' else []
        run([*command, build_target, '--profile', 'release', '--package', '--release-version', version, *extra])
        primary, _, _ = package_info(platform, arch, version)
        asset(primary, platform, arch)
        if platform == 'windows':
            asset(ROOT / '.build/packages/pixels-agent-bridge-windows-x86_64-release.zip', platform, arch,
                  f'pixels-agent-bridge-windows-x86_64-release-{version}.zip')
        elif platform == 'macos':
            asset(ROOT / f'.build/packages/pixels-agent-bridge-macos-{arch}-release.tar.gz', platform, arch,
                  f'pixels-agent-bridge-macos-{arch}-release-{version}.tar.gz')
        elif platform == 'linux':
            run([sys.executable, 'scripts/build_linux_desktop.py', '--version', version, '--package',
                 '--headless-package', primary, '--headless-sha256', digest(primary)])
            desktop, _, _ = package_info('linux-desktop', arch, version)
            asset(desktop, 'linux-desktop', arch)
    if source_state() != before:
        raise ValueError('Sources changed while building; discard this candidate')
    output = ROOT / '.build/github-release/out' / target
    output.mkdir(parents=True, exist_ok=False)
    for path, metadata in assets:
        shutil.copy2(path, output / metadata['name'])
        if digest(output / metadata['name']) != metadata['sha256']:
            raise ValueError('Artifact changed during staging')
    write_json(output / f'candidate-{target}.json', dict(schema_version=1, target=target,
               tag=tag, product=product, version=version, source=before, assets=[a for _, a in assets]))
    return output


def safe_name(name):
    if not isinstance(name, str) or not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]*', name):
        raise ValueError('Unsafe release asset name')
    if name in ('release.json', 'SHA256SUMS') or name.startswith('candidate-'):
        raise ValueError('Release asset collides with a control manifest')
    return name


def canonical(value):
    return DOMAIN + json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def signing_key(product):
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    name = 'PAB_RELEASE_PRIVATE_KEY_BASE64' if product == 'client' else 'PAB_SERVICE_RELEASE_PRIVATE_KEY_BASE64'
    try:
        return Ed25519PrivateKey.from_private_bytes(base64.b64decode(os.environ[name], validate=True))
    except (KeyError, ValueError):
        raise ValueError(f'Set the existing signing key in {name}') from None


def assemble(tag, directory):
    value, directory = plan(tag), Path(directory).resolve()
    source = checked_source(tag)
    expected = {t['id'] for t in value['matrix']['include']}
    manifests = list(directory.glob('candidate-*.json'))
    receipts = [json.loads(p.read_text(encoding='utf-8')) for p in manifests]
    if len(receipts) != len(expected) or {r['target'] for r in receipts} != expected:
        raise ValueError('Missing, duplicate or unexpected platform candidates')
    assets, names = [], set()
    for receipt in receipts:
        if (receipt.get('schema_version'), receipt['tag'], receipt['product'], receipt['version'], receipt['source']) != (
                1, tag, value['product'], value['version'], source):
            raise ValueError('Platform candidates do not share the tagged source and version')
        if not receipt['assets']:
            raise ValueError('Empty platform candidate')
        for item in receipt['assets']:
            name = safe_name(item['name'])
            path = directory / name
            if (name in names or path.is_symlink() or not path.is_file()
                    or digest(path) != item['sha256'] or path.stat().st_size != item['bytes']):
                raise ValueError('Candidate file is missing, duplicated or modified')
            names.add(name)
            assets.append(dict(item, target=receipt['target']))
    allowed = names | {p.name for p in manifests}
    if (directory / 'release.json').exists():
        if {p.name for p in directory.iterdir()} != allowed | {'release.json', 'SHA256SUMS'}:
            raise ValueError('Incomplete or unexpected existing release bundle')
        return read_bundle(tag, directory)
    if {p.name for p in directory.iterdir()} != allowed:
        raise ValueError('Unexpected file in candidate bundle')
    release = dict(schema_version=1, tag=tag, product=value['product'], version=value['version'],
                   source=source, assets=sorted(assets, key=lambda a: a['name']))
    key = signing_key(value['product'])
    write_json(directory / 'release.json', {'domain': DOMAIN.decode().strip(), 'release': release,
               'signature': base64.b64encode(key.sign(canonical(release))).decode()})
    (directory / 'SHA256SUMS').write_text(''.join(f"{a['sha256']}  {a['name']}\n" for a in release['assets']), encoding='utf-8')
    # Platform receipts are Actions artifacts, never release attachments.
    return release


def read_bundle(tag, directory):
    from cryptography.exceptions import InvalidSignature
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    directory = Path(directory).resolve()
    envelope = json.loads((directory / 'release.json').read_text(encoding='utf-8'))
    value = envelope['release']
    if envelope['domain'] != DOMAIN.decode().strip() or value['tag'] != tag:
        raise ValueError('Release manifest identity mismatch')
    if value['source'] != checked_source(tag) or value['version'] != plan(tag)['version'] or value['product'] != plan(tag)['product']:
        raise ValueError('Release manifest does not match the current tagged source')
    public_name = 'PAB_RELEASE_PUBLIC_KEY_BASE64' if value['product'] == 'client' else 'PAB_SERVICE_RELEASE_PUBLIC_KEY_BASE64'
    try:
        public = (Ed25519PublicKey.from_public_bytes(base64.b64decode(os.environ[public_name], validate=True))
                  if os.environ.get(public_name) else signing_key(value['product']).public_key())
        public.verify(base64.b64decode(envelope['signature'], validate=True), canonical(value))
    except InvalidSignature:
        raise ValueError('Release signature verification failed') from None
    for item in value['assets']:
        path = directory / safe_name(item['name'])
        if path.is_symlink() or not path.is_file() or digest(path) != item['sha256'] or path.stat().st_size != item['bytes']:
            raise ValueError('Release asset differs from signed manifest')
    sums = ''.join(f"{a['sha256']}  {a['name']}\n" for a in value['assets'])
    if (directory / 'SHA256SUMS').read_text(encoding='utf-8') != sums:
        raise ValueError('Checksum file differs from signed manifest')
    return value


def gh(*args, json_output=False):
    data = subprocess.check_output(['gh', *map(str, args)], cwd=ROOT, timeout=1200)
    return json.loads(data) if json_output else data


def remote_identity(tag, repo, commit):
    from urllib.parse import quote
    if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
        raise ValueError('Use GitHub OWNER/REPO')
    actual = gh('api', f'repos/{repo}/commits/{quote(tag, safe="")}', '--jq', '.sha').decode().strip()
    if actual != commit:
        raise ValueError('Remote tag does not match the tested commit')


def release_files(value, directory):
    return [Path(directory) / name for name in ['release.json', 'SHA256SUMS', *[a['name'] for a in value['assets']]]]


def verify_remote(value, directory, repo, draft=None, allow_partial=False):
    release = gh('release', 'view', value['tag'], '--repo', repo, '--json', 'tagName,isDraft,assets', json_output=True)
    expected = {p.name: p for p in release_files(value, directory)}
    actual = {a['name'] for a in release['assets']}
    if (release['tagName'] != value['tag'] or (draft is not None and release['isDraft'] != draft)
            or not actual <= expected.keys() or (not allow_partial and actual != expected.keys())):
        raise ValueError('Remote release state or attachment list differs')
    remote_identity(value['tag'], repo, value['source']['commit'])
    with tempfile.TemporaryDirectory(prefix='pab-release-verify-') as temporary:
        for name in sorted(actual):
            gh('release', 'download', value['tag'], '--repo', repo, '--dir', temporary, '--pattern', name)
            if digest(Path(temporary) / name) != digest(expected[name]):
                raise ValueError('Remote attachment differs; refusing to overwrite it')
    return actual


def draft(tag, directory, repo):
    value = read_bundle(tag, directory)
    remote_identity(tag, repo, value['source']['commit'])
    # Distinguish a missing release from authentication, networking and other API errors.
    from urllib.parse import quote
    result = subprocess.run(['gh', 'api', f'repos/{repo}/releases/tags/{quote(tag, safe="")}'],
                            cwd=ROOT, capture_output=True, text=True, timeout=60)
    if result.returncode:
        if '(HTTP 404)' not in result.stderr:
            raise ValueError('Cannot determine whether the release already exists')
        gh('release', 'create', tag, '--repo', repo, '--verify-tag', '--draft', '--latest=false',
           '--title', tag, '--notes', 'Candidate packages. Full release smoke qualification is required before publication.')
    existing = verify_remote(value, directory, repo, draft=True, allow_partial=True)
    for path in release_files(value, directory):
        if path.name not in existing:
            gh('release', 'upload', tag, path, '--repo', repo)
    verify_remote(value, directory, repo, draft=True)


def validate_reports(value, directory, reports):
    by_hash = {}
    paths = reports if isinstance(reports, (list, tuple)) else Path(reports).rglob('report.json')
    for path in paths:
        report = json.loads(path.read_text(encoding='utf-8'))
        sha = report.get('artifact', {}).get('sha256')
        if sha in by_hash:
            raise ValueError('Duplicate smoke reports for one artifact')
        by_hash[sha] = path
    for asset in value['assets']:
        if asset['sha256'] not in by_hash:
            raise ValueError(f"Missing full release smoke report for {asset['name']}")
        validate_release_report(by_hash[asset['sha256']], platform=asset['platform'], arch=asset['architecture'],
                                artifact=Path(directory) / asset['name'], version=value['version'],
                                commit=value['source']['commit'])


def publish(tag, directory, repo, reports):
    value = read_bundle(tag, directory)
    # Full existing gate, before signing/publication/network writes; no CI shortcut.
    validate_reports(value, directory, reports)
    verify_remote(value, directory, repo)
    state = gh('release', 'view', tag, '--repo', repo, '--json', 'isDraft', json_output=True)
    if not state['isDraft']:
        # Recover an interrupted successful publication without editing released bytes.
        return
    gh('release', 'edit', tag, '--repo', repo, '--draft=false',
       '--latest=' + str(plan(tag)['latest']).lower(),
       '--notes', f"{tag}\n\nCommit: {value['source']['commit']}\n\nAll attached candidates passed matching release smoke. SHA256SUMS and the signed release.json identify the exact tested files.")
    verify_remote(value, directory, repo, draft=False)


def qualify(tag, directory, target, lab):
    # Executed only on explicitly configured release-test runners, never public PR runners.
    value = plan(tag)
    policy = tomllib.loads(Path(lab).read_text(encoding='utf-8'))
    entry = policy['targets'][target]
    if not isinstance(entry.get('reports'), list) or not entry['reports']:
        raise ValueError('Configure real completed release reports in the private lab TOML')
    selected = json.loads((Path(directory) / f'candidate-{target}.json').read_text(encoding='utf-8'))
    if selected['tag'] != tag or selected['source'] != checked_source(tag):
        raise ValueError('Lab candidates do not match the tagged sources')
    # The lab owns running smoke/run.py and real observations. Never synthesize a pass.
    reduced = dict(value, source=selected['source'], assets=selected['assets'])
    reports = ROOT / 'smoke/runs'
    paths = [(ROOT / p).resolve() for p in entry['reports']]
    if any(not p.is_relative_to(reports.resolve()) or p.name != 'report.json' for p in paths):
        raise ValueError('Lab reports must remain under smoke/runs')
    validate_reports(reduced, directory, paths)
    run_id = os.environ.get('GITHUB_RUN_ID', 'local')
    if run_id != 'local' and not run_id.isdigit():
        raise ValueError('Invalid qualification run ID')
    export = ROOT / '.build/github-release/qualification' / run_id / target
    for asset in selected['assets']:
        matches = [p for p in paths
                   if json.loads(p.read_text(encoding='utf-8')).get('artifact', {}).get('sha256') == asset['sha256']]
        report_path = matches[0]
        report = json.loads(report_path.read_text(encoding='utf-8'))
        # Export only gate inputs, never logs, configuration, captured desktops or credentials.
        files = [report_path, *[(report_path.parent / c['attempts'][-1]['evidence']).resolve() for c in report['cases']]]
        for path in files:
            no_credentials(json.loads(path.read_text(encoding='utf-8')))
            destination = export / path.relative_to(reports)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, destination)


def no_credentials(value):
    if isinstance(value, dict):
        for key, item in value.items():
            if key.lower() in ('password', 'access_token', 'refresh_token', 'private_key', 'authorization') and item:
                raise ValueError('Do not export credential fields in release evidence')
            no_credentials(item)
    elif isinstance(value, list):
        for item in value:
            no_credentials(item)
    elif isinstance(value, str) and re.search(r'PRIVATE KEY-----|\bgh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}', value):
        raise ValueError('Do not export secrets in release evidence')


def context(path, repo, output):
    value = json.loads(Path(path).read_text(encoding='utf-8'))
    plan(value['tag'])
    run = trusted_run(repo, value['candidate_run'], 'release.yml', value['tag'])
    if value['commit'] != run['head_sha']:
        raise ValueError('Qualification context differs from the candidate workflow')
    with Path(output).open('a', encoding='utf-8') as stream:
        for key in ('tag', 'candidate_run', 'commit'):
            stream.write(f'{key}={value[key]}\n')


def trusted_run(repo, run_id, workflow, tag=None):
    if not str(run_id).isdigit():
        raise ValueError('Use a numeric Actions run ID')
    run = gh('api', f'repos/{repo}/actions/runs/{run_id}', json_output=True)
    if (run['repository']['full_name'] != repo or run['head_repository']['full_name'] != repo
            or run['path'] != '.github/workflows/' + workflow or run['conclusion'] != 'success'
            or run['event'] not in ('push', 'workflow_dispatch')):
        raise ValueError('Only successful trusted release workflow runs can supply candidates or evidence')
    if tag:
        remote_identity(tag, repo, run['head_sha'])
    return run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    for action in ('plan', 'preflight', 'build', 'assemble', 'draft', 'publish', 'qualify', 'verify'):
        p = sub.add_parser(action)
        p.add_argument('--tag', required=True)
        if action in ('build', 'qualify'):
            p.add_argument('--target', required=True)
        if action in ('assemble', 'draft', 'publish', 'qualify', 'verify'):
            p.add_argument('--directory', type=Path, required=True)
        if action in ('draft', 'publish', 'verify'):
            p.add_argument('--repo', required=True)
        if action == 'publish':
            p.add_argument('--reports', type=Path, required=True)
        if action == 'qualify':
            p.add_argument('--lab', type=Path, required=True)
        if action == 'plan':
            p.add_argument('--github-output', type=Path)
    p = sub.add_parser('check-run')
    p.add_argument('--repo', required=True)
    p.add_argument('--run-id', required=True)
    p.add_argument('--workflow', choices=('release.yml', 'release-smoke.yml'), required=True)
    p.add_argument('--tag')
    p = sub.add_parser('context')
    p.add_argument('--file', type=Path, required=True)
    p.add_argument('--repo', required=True)
    p.add_argument('--github-output', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'context':
        context(args.file, args.repo, args.github_output)
    elif args.action == 'check-run':
        trusted_run(args.repo, args.run_id, args.workflow, args.tag)
    elif args.action == 'preflight':
        preflight(args.tag)
    elif args.action == 'plan':
        value = plan(args.tag)
        if args.github_output:
            checked_source(args.tag)
            with args.github_output.open('a', encoding='utf-8') as output:
                for key in ('tag', 'product', 'version', 'matrix'):
                    output.write(f'{key}={json.dumps(value[key], separators=(",", ":")) if key == "matrix" else value[key]}\n')
        print(json.dumps(value))
    elif args.action == 'build':
        print(build(args.tag, args.target))
    elif args.action == 'assemble':
        assemble(args.tag, args.directory)
    elif args.action == 'draft':
        draft(args.tag, args.directory, args.repo)
    elif args.action == 'publish':
        publish(args.tag, args.directory, args.repo, args.reports)
    elif args.action == 'qualify':
        qualify(args.tag, args.directory, args.target, args.lab)
    elif args.action == 'verify':
        verify_remote(read_bundle(args.tag, args.directory), args.directory, args.repo)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # Never print environment contents, secret-bearing argv or raw library errors.
        print(f'GitHub release failed: {error}' if isinstance(error, ValueError)
              else 'GitHub release failed; inspect the preceding stage and its exit status', file=sys.stderr)
        sys.exit(1)
