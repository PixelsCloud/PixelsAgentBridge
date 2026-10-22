"""Assemble a publishable DeepSeek Harness plugin directory (and tarball).

The plugin source lives in apps/dsh-pab and deliberately carries no binary in
git. A release instead ships a self-contained directory whose native/ entry was
taken from the freshly built pab-mcp, so an operator installs one folder and gets
the control panel without the Pixels desktop app.

Usage:
    python scripts/package_dsh_plugin.py --profile release
    python scripts/package_dsh_plugin.py --profile release --build --version 0.2.0
    python scripts/package_dsh_plugin.py --platform darwin-arm64 --binary path/to/pab-mcp

Output (default):
    .build/packages/dsh-pab-<version>/        complete install directory
    .build/packages/dsh-pab-<version>.tgz     npm tarball of the same tree
"""
from argparse import ArgumentParser, RawDescriptionHelpFormatter
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'apps' / 'dsh-pab'
CARGO_TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve()
CARGO_JOBS = max(1, min(10, int(os.environ.get('CARGO_BUILD_JOBS', '10'))))

PACKAGE_FILES = ['package.json', 'cordis.patch.yml', 'README.md']
SOURCE_FILES = ['src/index.js', 'src/client.js', 'src/ui.js']


def host_platform():
    machine = platform.machine().lower()
    arch = 'arm64' if machine in ('arm64', 'aarch64') else 'x64'
    if sys.platform == 'win32':
        return f'win32-{arch}'
    if sys.platform == 'darwin':
        return f'darwin-{arch}'
    return f'linux-{arch}'


def binary_name(platform_id):
    return 'pab-mcp.exe' if platform_id.startswith('win32') else 'pab-mcp'


def build_binary(profile):
    flags = ['--release'] if profile == 'release' else []
    command = ['cargo', 'build', '--locked', *flags, '-p', 'pab-bridge', '--bin', 'pab-mcp', '-j', str(CARGO_JOBS)]
    print('> ' + ' '.join(command), flush=True)
    subprocess.run(command, cwd=ROOT, check=True)


def resolve_binary(args, platform_id):
    if args.binary:
        binary = Path(args.binary).resolve()
        if not binary.is_file():
            raise SystemExit(f'--binary is not a file: {binary}')
        return binary

    binary = CARGO_TARGET / args.profile / binary_name(platform_id)
    if args.build and not binary.is_file():
        build_binary(args.profile)
    if not binary.is_file():
        raise SystemExit(
            f'pab-mcp not found at {binary}. Build it first (python scripts/package_dsh_plugin.py --build) '
            'or pass --binary with a cross-built artifact.'
        )
    return binary


def plugin_version():
    return json.loads((PLUGIN / 'package.json').read_text(encoding='utf-8'))['version']


def assemble(stage, binary, platform_id, version):
    stage.mkdir(parents=True, exist_ok=False)

    manifest = json.loads((PLUGIN / 'package.json').read_text(encoding='utf-8'))
    manifest['version'] = version
    (stage / 'package.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')

    for name in PACKAGE_FILES[1:]:
        shutil.copy2(PLUGIN / name, stage / name)
    for name in ('LICENSE', 'NOTICE', 'COMMERCIAL_LICENSE.md'):
        shutil.copy2(ROOT / name, stage / name)
    for name in SOURCE_FILES:
        target = stage / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(PLUGIN / name, target)
    if (PLUGIN / 'assets').is_dir():
        shutil.copytree(PLUGIN / 'assets', stage / 'assets')

    native = stage / 'native' / platform_id
    native.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, native / binary_name(platform_id))

    return manifest


def sha256(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def write_manifest(stage, binary, platform_id, repo_version):
    record = {
        'package': 'dsh-pab',
        'platform': platform_id,
        'repoVersion': repo_version,
        'builtAt': datetime.now(timezone.utc).isoformat(timespec='seconds'),
        'binary': {
            'name': binary_name(platform_id),
            'bytes': binary.stat().st_size,
            'sha256': sha256(binary),
        },
    }
    (stage / 'release-manifest.json').write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')
    return record


def repo_version():
    try:
        return json.loads((ROOT / 'build-version.json').read_text(encoding='utf-8'))['version']
    except (OSError, KeyError, ValueError):
        return None


def pack(stage, out):
    npm = shutil.which('npm')
    if not npm:
        print('npm not found; skipping tarball', file=sys.stderr)
        return None
    command = [npm, 'pack', '--pack-destination', str(out)]
    print('> ' + ' '.join(command), flush=True)
    result = subprocess.run(command, cwd=stage, text=True, capture_output=True)
    if result.returncode != 0:
        print(result.stdout + result.stderr, file=sys.stderr)
        raise SystemExit('npm pack failed')
    name = [line.strip() for line in result.stdout.splitlines() if line.strip().endswith('.tgz')]
    return out / name[-1] if name else None


def main():
    parser = ArgumentParser(description=__doc__, formatter_class=RawDescriptionHelpFormatter)
    parser.add_argument('--profile', choices=['debug', 'release'], default='release')
    parser.add_argument('--platform', default=None, help='target platform id, for example win32-x64 (default: host)')
    parser.add_argument('--binary', help='path to a pab-mcp binary, for example a cross-built artifact')
    parser.add_argument('--version', help='package version for this release (default: apps/dsh-pab/package.json)')
    parser.add_argument('--out', default=str(ROOT / '.build' / 'packages'), help='output directory')
    parser.add_argument('--build', action='store_true', help='build pab-mcp with cargo before packaging')
    parser.add_argument('--no-tgz', action='store_true', help='produce only the directory, no npm tarball')
    args = parser.parse_args()

    platform_id = args.platform or host_platform()
    version = args.version or plugin_version()
    binary = resolve_binary(args, platform_id)

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    stage = out / f'dsh-pab-{version}'
    if stage.exists():
        shutil.rmtree(stage)

    assemble(stage, binary, platform_id, version)
    record = write_manifest(stage, binary, platform_id, repo_version())

    tarball = None if args.no_tgz else pack(stage, out)

    print('', flush=True)
    print(f'Release directory: {stage}', flush=True)
    print(f'  version      {version}', flush=True)
    print(f'  platform    {platform_id}', flush=True)
    print(f'  pab-mcp     {record["binary"]["bytes"]} bytes, sha256 {record["binary"]["sha256"][:16]}...', flush=True)
    print(f'  repo build  {record["repoVersion"]}', flush=True)
    for path in sorted(stage.rglob('*')):
        if path.is_file() and path.name != 'release-manifest.json':
            print(f'  {path.relative_to(stage).as_posix()}', flush=True)
    if tarball:
        print(f'Tarball: {tarball}', flush=True)
    print('', flush=True)
    print('Install on the operator machine:', flush=True)
    print(f'  dsh plugin --profile desktop add {stage}', flush=True)
    print('  then restart DeepSeek Harness', flush=True)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Packaging failed: {error}', file=sys.stderr)
        sys.exit(1)
