"""Package management or Relay independently; retain the legacy combined server bundle."""
from hashlib import sha256
import json
import tarfile
import zipfile

from build_version import verify_artifacts
from service_products import artifacts


def deployment_files(root, product, host):
    files = {name: root / name for name in ('LICENSE', 'NOTICE', 'COMMERCIAL_LICENSE.md', 'BUILDING.md')}
    if product in ('management', 'server'):
        files['WEB_DEPLOYMENT.md'] = root / 'docs/WEB_DEPLOYMENT.md'
        files['GITHUB_LOGIN_SETUP.md'] = root / 'docs/GITHUB_LOGIN_SETUP.md'
        files['cluster.example.toml'] = root / 'packaging/server/cluster.example.toml'
    if product in ('relay', 'server'):
        for name in ('rollout.example.toml', 'healthcheck.example.toml'):
            files[name] = root / 'packaging/server' / name
        files['roll_relays.py'] = root / 'scripts/roll_relays.py'
        files['RELAY_CLUSTER_OPERATIONS.md'] = root / 'docs/RELAY_CLUSTER_OPERATIONS.md'
        files['docker/install.py'] = root / 'packaging/server/docker/install.py'
    if host == 'windows':
        files['windows/install.ps1'] = root / 'packaging/server/windows/install.ps1'
    else:
        files['linux/install.py'] = root / 'packaging/server/linux/install.py'
    for component in ('server', 'relay'):
        if product == 'server' or (component == 'server' and product == 'management') or component == product:
            name = component + '.example.toml'
            member = 'windows/' + name if host == 'windows' else name
            files[member] = root / 'packaging/server/windows' / name
    return files


def package(root, product, host, profile, binary_dir, output_dir, architecture):
    files = artifacts(root, product, host, binary_dir)
    version = verify_artifacts(root, product, profile, files)
    receipt = json.loads((root / f'.build/builds/{product}-{profile}.json').read_text(encoding='utf-8'))
    binaries = set(files) - {name for name in files if name.startswith('web/')}
    files.update(deployment_files(root, product, host))
    for name, path in files.items():
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'Missing required package input: {name}')
    output_dir.mkdir(parents=True, exist_ok=True)
    name = f'pixels-agent-bridge-{product}-{host}-{architecture}-{profile}'
    # Preserve the old combined archive name for existing rollout consumers.
    if product != 'server':
        name += '-' + version
    archive = output_dir / (name + ('.zip' if host == 'windows' else '.tar.gz'))
    if host == 'windows':
        with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as out:
            for member, path in files.items():
                out.write(path, member)
    else:
        with tarfile.open(archive, 'w:gz') as out:
            for member, path in files.items():
                def permissions(info):
                    info.mode = 0o755 if member in binaries else 0o644
                    return info
                out.add(path, arcname=member, recursive=False, filter=permissions)
    manifest = {'archive': archive.name, 'kind': 'native', 'product': product, 'platform': host, 'architecture': architecture,
                'source': receipt.get('source'),
                'version': version, 'sha256': sha256(archive.read_bytes()).hexdigest(),
                'files': {member: sha256(path.read_bytes()).hexdigest() for member, path in files.items()}}
    archive.with_suffix(archive.suffix + '.manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf8')
    return {'archive': str(archive), 'product': product, 'version': version,
            'files': len(files), 'sha256': manifest['sha256']}
