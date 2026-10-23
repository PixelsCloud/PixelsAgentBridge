"""Export a product image by immutable ID with a source-bound release manifest."""
import json
import subprocess

from build_version import digest, write_json


def package_image(root, product, version, image, source):
    info = json.loads(subprocess.check_output(['docker', 'image', 'inspect', image], text=True, timeout=60))[0]
    labels = info.get('Config', {}).get('Labels') or {}
    if (info['Os'] != 'linux' or info['Architecture'] not in ('amd64', 'arm64')
            or labels.get('vip.rgaa.pab.product') != product
            or labels.get('org.opencontainers.image.version') != version
            or (source is not None and (labels.get('org.opencontainers.image.revision') != source['commit']
                or labels.get('vip.rgaa.pab.source-sha256') != source['sha256']))):
        raise ValueError('Image identity does not match the built product, version and source')
    architecture = {'amd64': 'x86_64', 'arm64': 'aarch64'}[info['Architecture']]
    folder = root / '.build/packages'
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / f'pixels-agent-bridge-{product}-linux-{architecture}-image-release-{version}.tar'
    partial = path.with_suffix('.tar.tmp')
    subprocess.run(['docker', 'image', 'save', '--output', str(partial), info['Id']], check=True, timeout=1200)
    partial.replace(path)
    metadata = {'archive': path.name, 'kind': 'docker-image', 'product': product, 'platform': 'linux',
                'architecture': architecture, 'version': version, 'source': source,
                'sha256': digest(path), 'image_id': info['Id'], 'image_tag': image}
    write_json(path.with_suffix('.tar.manifest.json'), metadata)
    return path
