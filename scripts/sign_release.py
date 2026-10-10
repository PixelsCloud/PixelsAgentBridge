"""Print the offline signature for a manually built release installer.

Use the exact version reserved in the website admin page. This command only
reads the installer and the private key; it does not upload or publish.
"""

from argparse import ArgumentParser
from base64 import b64encode
from pathlib import Path
import os
import re

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from release import canonical, digest


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('installer', type=Path)
    args = parser.parse_args()
    match = re.fullmatch(
        r'pixels-agent-bridge-(windows-x86_64|macos-aarch64)-release-(\d+\.\d+\.\d+)-setup\.(exe|pkg)',
        args.installer.name,
    )
    if not match or (match.group(1).startswith('windows') and match.group(3) != 'exe') or (
        match.group(1).startswith('macos') and match.group(3) != 'pkg'
    ):
        parser.error('Expected the versioned Windows EXE or macOS PKG filename')
    if not args.installer.is_file():
        parser.error('Installer file does not exist')
    key_path = os.environ.get('PAB_RELEASE_PRIVATE_KEY_FILE')
    if not key_path:
        parser.error('Set PAB_RELEASE_PRIVATE_KEY_FILE to the private Ed25519 key path')
    platform, arch = ('windows', 'x86_64') if match.group(1).startswith('windows') else ('macos', 'aarch64')
    key = Ed25519PrivateKey.from_private_bytes(Path(key_path).read_bytes())
    signature = key.sign(canonical(platform, arch, match.group(2), args.installer.name,
                                   args.installer.stat().st_size, digest(args.installer)))
    print(b64encode(signature).decode('ascii'))


if __name__ == '__main__':
    main()
