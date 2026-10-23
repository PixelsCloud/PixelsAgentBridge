"""Artifact boundaries for management, Relay and the legacy combined server bundle."""
from pathlib import Path

BINARIES = {
    'management': ('pab-server',),
    'relay': ('pab-relay-server', 'pab-relay-healthcheck'),
    'server': ('pab-server', 'pab-relay-server', 'pab-relay-healthcheck'),
}
WEB_PRODUCTS = frozenset(('management', 'server'))
WEB_EXTENSIONS = {'.html', '.js', '.css', '.svg', '.png', '.ico', '.woff2'}
WEB_LICENSES = frozenset(('license.txt', 'assets/OFL-10Pixel.txt'))


def artifacts(root, product, platform, binary_dir):
    """Use the same exact inputs for build receipts and archive verification."""
    suffix = '.exe' if platform == 'windows' else ''
    files = {name + suffix: Path(binary_dir) / (name + suffix) for name in BINARIES[product]}
    if product in WEB_PRODUCTS:
        web = Path(root) / 'apps/web/dist'
        if web.is_symlink() or not (web / 'index.html').is_file():
            raise ValueError('Build apps/web first; a regular production Web directory is required')
        for path in sorted(web.rglob('*')):
            if path.is_symlink():
                raise ValueError(f'Symlinks are not production Web assets: {path.name}')
            if path.is_file():
                relative = path.relative_to(web).as_posix()
                if path.suffix not in WEB_EXTENSIONS and relative not in WEB_LICENSES:
                    raise ValueError(f'Unexpected production Web asset: {path.name}')
                files['web/' + relative] = path
    for path in files.values():
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'Missing or non-regular build artifact: {path.name}')
    return files
