"""Package built Server/Relay binaries and the production Web bundle (no credentials)."""
from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import json
import tarfile
import zipfile

root = Path(__file__).resolve().parents[2]
parser = ArgumentParser(description=__doc__)
parser.add_argument('--platform', choices=['windows', 'linux'], required=True)
parser.add_argument('--profile', choices=['debug', 'release'], default='release')
parser.add_argument('--binary-dir', type=Path)
parser.add_argument('--output-dir', type=Path, default=root / '.build/packages')
args = parser.parse_args()
binary_dir = args.binary_dir or root / 'target' / args.profile
suffix = '.exe' if args.platform == 'windows' else ''
files = [(binary_dir / (name + suffix), name + suffix) for name in ['pab-server', 'pab-relay-server']]
web = root / 'apps/web/dist'
if not (web / 'index.html').is_file():
    raise SystemExit('Build apps/web first: npm ci && npm run build')
allowed = {'.html', '.js', '.css', '.svg', '.png', '.ico', '.woff2'}
for path in sorted(web.rglob('*')):
    if path.is_file():
        if path.is_symlink() or path.suffix not in allowed:
            raise SystemExit(f'Unexpected production Web asset: {path.name}')
        files.append((path, 'web/' + path.relative_to(web).as_posix()))
files.append((root / 'WEB_DEPLOYMENT.md', 'WEB_DEPLOYMENT.md'))
files.append((root / 'WEB_DEVELOPMENT.md', 'WEB_DEVELOPMENT.md'))
for path, _ in files:
    if not path.is_file():
        raise SystemExit(f'Missing required build artifact: {path}')
args.output_dir.mkdir(parents=True, exist_ok=True)
name = f'pixels-agent-bridge-server-{args.platform}-x86_64-{args.profile}'
archive = args.output_dir / (name + ('.zip' if args.platform == 'windows' else '.tar.gz'))
if args.platform == 'windows':
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as out:
        for path, member in files:
            out.write(path, member)
else:
    with tarfile.open(archive, 'w:gz') as out:
        for path, member in files:
            def permissions(info):
                info.mode = 0o755 if member in ['pab-server', 'pab-relay-server'] else 0o644
                return info
            out.add(path, arcname=member, recursive=False, filter=permissions)
manifest = {'archive': archive.name, 'sha256': sha256(archive.read_bytes()).hexdigest(), 'files': {member: sha256(path.read_bytes()).hexdigest() for path, member in files}}
archive.with_suffix(archive.suffix + '.manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
print(json.dumps({'archive': str(archive), 'files': len(files), 'sha256': manifest['sha256']}))
