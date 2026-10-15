"""Copy the canonical one-line Linux installer into the website's static assets."""
import argparse
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--site', type=Path, default=ROOT.parent / 'PixelsAgentBridgeSite')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    source = (ROOT / 'packaging/desktop/linux-bootstrap.sh').read_bytes().replace(b'\r\n', b'\n')
    destination = args.site / 'web/public/install.sh'
    if args.check:
        if not destination.is_file() or destination.read_bytes().replace(b'\r\n', b'\n') != source:
            parser.error('Website install.sh is stale; run scripts/sync_linux_bootstrap.py')
    else:
        if not destination.parent.is_dir():
            parser.error('Website web/public directory does not exist')
        destination.write_bytes(source)
    print('Linux bootstrap matches the website.' if args.check else str(destination))


if __name__ == '__main__':
    main()
