"""Package independent management/Relay products or the legacy combined bundle."""
from argparse import ArgumentParser
from pathlib import Path
import json
import platform
import sys

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'scripts'))
from package_services import package
from service_products import BINARIES
parser = ArgumentParser(description=__doc__)
parser.add_argument('--product', choices=tuple(BINARIES), default='server')
parser.add_argument('--platform', choices=['windows', 'linux'], required=True)
parser.add_argument('--profile', choices=['debug', 'release'], default='release')
parser.add_argument('--binary-dir', type=Path)
parser.add_argument('--output-dir', type=Path, default=root / '.build/packages')
args = parser.parse_args()
binary_dir = args.binary_dir or root / 'target' / args.profile
machine = platform.machine().lower()
architecture = {'amd64': 'x86_64', 'x86_64': 'x86_64', 'aarch64': 'aarch64', 'arm64': 'aarch64'}.get(machine)
if architecture is None:
    parser.error('Unsupported native service architecture: ' + machine)
print(json.dumps(package(root, args.product, args.platform, args.profile, binary_dir, args.output_dir, architecture)))
