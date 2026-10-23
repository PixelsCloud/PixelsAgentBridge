"""Smoke-gated management/Relay releases, isolated from the client update service."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory
import zipfile
from urllib.parse import quote

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from cryptography.exceptions import InvalidSignature

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from smoke.run import digest, source_state, validate_release_report
from build_version import parse_version, write_json

DOMAIN = b'PAB-SERVICE-RELEASE-V1\n'


def release_tag(metadata):
    suffix = '-image' if metadata['kind'] == 'docker-image' else ''
    return f'{metadata["product"]}/v{metadata["version"]}-{metadata["platform"]}-{metadata["architecture"]}{suffix}'


def read_candidate(artifact, product):
    artifact = Path(artifact).resolve(strict=True)
    metadata = json.loads(artifact.with_suffix(artifact.suffix + '.manifest.json').read_text(encoding='utf-8'))
    parse_version(metadata['version'])
    if (metadata.get('product') != product or product not in ('management', 'relay')
            or metadata.get('archive') != artifact.name or metadata.get('sha256') != digest(artifact)
            or metadata.get('platform') not in ('windows', 'linux')
            or metadata.get('architecture') not in ('x86_64', 'aarch64')):
        raise ValueError('Service artifact identity or checksum mismatch')
    expected = f'pixels-agent-bridge-{product}-{metadata["platform"]}-{metadata["architecture"]}-release-{metadata["version"]}'
    if metadata.get('kind') == 'docker-image':
        expected = expected.replace('-release-', '-image-release-') + '.tar'
        if metadata['platform'] != 'linux' or artifact.name != expected:
            raise ValueError('Image package identity mismatch')
        with tarfile.open(artifact) as archive:
            entries = json.load(archive.extractfile('manifest.json'))
            if len(entries) != 1:
                raise ValueError('Export exactly one product image')
            with archive.extractfile(entries[0]['Config']) as stream:
                configuration = stream.read(1024 * 1024)
            if 'sha256:' + hashlib.sha256(configuration).hexdigest() != metadata.get('image_id'):
                raise ValueError('Image archive configuration differs from the built image ID')
        return metadata
    extension = '.zip' if metadata['platform'] == 'windows' else '.tar.gz'
    if artifact.name != expected + extension or metadata.get('kind') != 'native':
        raise ValueError('A release service package with its own manifest is required')
    files = set(metadata['files'])
    suffix = '.exe' if metadata['platform'] == 'windows' else ''
    required = ({'pab-server' + suffix, 'web/index.html'} if product == 'management' else
                {'pab-relay-server' + suffix, 'pab-relay-healthcheck' + suffix})
    forbidden = ({'pab-relay-server', 'pab-relay-healthcheck'} if product == 'management' else {'pab-server'})
    forbidden |= {'pab-desktop', 'pab-executor', 'pab-mcp'}
    if (not required <= files or any(Path(name).stem in forbidden for name in files)
            or (product == 'relay' and any(name.startswith('web/') for name in files))):
        raise ValueError('Service archive does not contain its own product')
    # Read member contents without extracting any archive-supplied paths.
    if extension == '.zip':
        with zipfile.ZipFile(artifact) as archive:
            members = archive.infolist()
            if len(members) != len(metadata['files']) or {m.filename for m in members} != set(metadata['files']):
                raise ValueError('Service archive member list mismatch')
            for member in members:
                if stat.S_ISLNK(member.external_attr >> 16):
                    raise ValueError('Service archive must contain regular files')
                with archive.open(member) as stream:
                    if hashlib.file_digest(stream, 'sha256').hexdigest() != metadata['files'][member.filename]:
                        raise ValueError('Service archive member checksum mismatch')
    else:
        with tarfile.open(artifact, 'r:gz') as archive:
            members = archive.getmembers()
            if len(members) != len(metadata['files']) or {m.name for m in members} != set(metadata['files']):
                raise ValueError('Service archive member list mismatch')
            for member in members:
                if not member.isfile():
                    raise ValueError('Service archive must contain regular files')
                with archive.extractfile(member) as stream:
                    if hashlib.file_digest(stream, 'sha256').hexdigest() != metadata['files'][member.name]:
                        raise ValueError('Service archive member checksum mismatch')
    return metadata


def prepare(product, artifact, notes):
    metadata = read_candidate(artifact, product)
    source = source_state()
    if source['dirty'] or source != metadata.get('source'):
        raise ValueError('Candidate must come from the current committed sources; rebuild it')
    folder = ROOT / '.build/service-releases' / product / metadata['version'] / (metadata['platform'] + '-' + metadata['architecture']) / metadata['kind']
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / 'state.json'
    if path.exists():
        raise ValueError('Candidate state already exists; use publish or verify instead of overwriting it')
    # Freeze deployable bytes and public notes before testing/publishing.
    copied = folder / metadata['archive']
    shutil.copyfile(artifact, copied)
    if digest(copied) != metadata['sha256']:
        raise ValueError('Artifact changed while preparing the candidate')
    shutil.copyfile(notes, folder / 'notes.md')
    tag = release_tag(metadata)
    state = {'phase': 'prepared', 'product': product, 'metadata': metadata, 'artifact': str(copied),
             'notes_sha256': digest(folder / 'notes.md'), 'tag': tag}
    write_json(path, state)
    return path


def read_state(path):
    path = Path(path).resolve(strict=True)
    if path.name != 'state.json' or not path.is_relative_to((ROOT / '.build/service-releases').resolve()):
        raise ValueError('Use the state.json returned by service release prepare')
    state = json.loads(path.read_text(encoding='utf-8'))
    metadata = state['metadata']
    artifact = Path(state['artifact']).resolve(strict=True)
    expected_tag = release_tag(metadata)
    if (state['product'] not in ('management', 'relay') or state['product'] != metadata['product']
            or state['tag'] != expected_tag or artifact.parent != path.parent
            or artifact.name != metadata['archive'] or digest(artifact) != metadata['sha256']):
        raise ValueError('Service candidate state or artifact changed')
    return path, state, artifact


def public_manifest(state, artifact, report):
    value = dict(state['metadata'], bytes=artifact.stat().st_size, tag=state['tag'],
                 smoke={'sha256': digest(report), 'profile': json.loads(Path(report).read_text(encoding='utf-8'))['profile']})
    key_file = os.environ.get('PAB_SERVICE_RELEASE_PRIVATE_KEY_FILE')
    if not key_file:
        raise ValueError('Set PAB_SERVICE_RELEASE_PRIVATE_KEY_FILE privately before publishing')
    key = Ed25519PrivateKey.from_private_bytes(Path(key_file).read_bytes())
    encoded = json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode('utf-8')
    return {'schema_version': 1, 'domain': DOMAIN.decode().strip(), 'release': value,
            'signature': base64.b64encode(key.sign(DOMAIN + encoded)).decode()}


def verify_file(artifact, manifest, public_key):
    """The caller supplies a separately trusted 32-byte public key, never the download's own key."""
    envelope = json.loads(Path(manifest).read_text(encoding='utf-8'))
    if envelope.get('schema_version') != 1 or envelope.get('domain') != DOMAIN.decode().strip():
        raise ValueError('Unsupported service signature domain')
    metadata = envelope['release']
    encoded = json.dumps(metadata, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode('utf-8')
    try:
        Ed25519PublicKey.from_public_bytes(Path(public_key).read_bytes()).verify(
            base64.b64decode(envelope['signature'], validate=True), DOMAIN + encoded)
    except InvalidSignature:
        raise ValueError('Service release signature verification failed') from None
    artifact = Path(artifact)
    if (metadata.get('product') not in ('management', 'relay') or metadata.get('archive') != artifact.name
            or metadata.get('bytes') != artifact.stat().st_size or metadata.get('sha256') != digest(artifact)):
        raise ValueError('Downloaded artifact differs from the signed service release')
    return metadata


def verify(path, repo):
    path, state, artifact = read_state(path)
    if state['phase'] not in ('publishing', 'published') or state.get('repo') != repo:
        raise ValueError('No publication to verify for this repository')
    envelope = path.parent / 'release.json'
    if digest(envelope) != state['release_sha256']:
        raise ValueError('Frozen signed release manifest changed')
    release = json.loads(subprocess.check_output(['gh', 'release', 'view', state['tag'], '--repo', repo,
                          '--json', 'tagName,targetCommitish,isDraft,assets'], cwd=ROOT, timeout=60))
    if (release['tagName'] != state['tag'] or release['targetCommitish'] != state['metadata']['source']['commit']
            or release['isDraft'] or {a['name'] for a in release['assets']} != {artifact.name, 'release.json'}):
        raise ValueError('Remote release identity, state or assets differ; reconcile it manually')
    # targetCommitish is release metadata; resolve the actual tag as well.
    actual_commit = subprocess.check_output(['gh', 'api', f'repos/{repo}/commits/{quote(state["tag"], safe="")}',
                                             '--jq', '.sha'], cwd=ROOT, timeout=60).decode().strip()
    if actual_commit != state['metadata']['source']['commit']:
        raise ValueError('Remote Git tag points to a different commit; reconcile it manually')
    with TemporaryDirectory(prefix='verify-', dir=path.parent) as directory:
        subprocess.run(['gh', 'release', 'download', state['tag'], '--repo', repo, '--dir', directory,
                        '--pattern', artifact.name, '--pattern', 'release.json'], cwd=ROOT, check=True, timeout=900)
        if digest(Path(directory) / artifact.name) != state['metadata']['sha256'] or digest(Path(directory) / 'release.json') != state['release_sha256']:
            raise ValueError('Downloaded release assets do not match the approved candidate')
    state['phase'] = 'published'
    write_json(path, state)


def publish(path, report, repo):
    path, state, artifact = read_state(path)
    if state['phase'] != 'prepared':
        raise ValueError('Publication has already been attempted; use verify, do not replay it')
    metadata = state['metadata']
    # Gate runs before any remote request, signing or publication side effect.
    validate_release_report(report, platform=state['product'], arch=metadata['architecture'],
                            artifact=artifact, version=metadata['version'], commit=metadata['source']['commit'])
    if source_state() != metadata['source'] or digest(path.parent / 'notes.md') != state['notes_sha256']:
        raise ValueError('Candidate source or release notes changed')
    write_json(path.parent / 'release.json', public_manifest(state, artifact, report))
    state.update(phase='publishing', repo=repo, release_sha256=digest(path.parent / 'release.json'))
    write_json(path, state)
    try:
        subprocess.run(['gh', 'release', 'create', state['tag'], str(artifact), str(path.parent / 'release.json'),
                        '--repo', repo, '--target', metadata['source']['commit'], '--latest=false',
                        '--title', state['tag'], '--notes-file', str(path.parent / 'notes.md')],
                       cwd=ROOT, check=True, timeout=1200)
    except (OSError, subprocess.SubprocessError):
        raise RuntimeError('Publication outcome is unconfirmed. Use verify; do not create another release blindly.') from None
    verify(path, repo)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    prepare_cmd = commands.add_parser('prepare')
    prepare_cmd.add_argument('product', choices=('management', 'relay'))
    prepare_cmd.add_argument('--artifact', type=Path, required=True)
    prepare_cmd.add_argument('--notes-file', type=Path, required=True)
    check_cmd = commands.add_parser('verify-file')
    check_cmd.add_argument('--artifact', type=Path, required=True)
    check_cmd.add_argument('--manifest', type=Path, required=True)
    check_cmd.add_argument('--public-key-file', type=Path, required=True, help='Separately trusted raw Ed25519 public key')
    for action in ('publish', 'verify'):
        command = commands.add_parser(action)
        command.add_argument('--state', type=Path, required=True)
        command.add_argument('--repo', required=True, help='GitHub OWNER/REPO; service tags never become Latest')
        if action == 'publish':
            command.add_argument('--smoke-report', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'prepare':
        print(prepare(args.product, args.artifact, args.notes_file))
    elif args.action == 'verify-file':
        metadata = verify_file(args.artifact, args.manifest, args.public_key_file)
        print(f'Verified {metadata["product"]} {metadata["version"]}: {metadata["archive"]}')
    else:
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', args.repo):
            parser.error('--repo must be OWNER/REPO')
        if args.action == 'publish':
            publish(args.state, args.smoke_report, args.repo)
        else:
            verify(args.state, args.repo)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
