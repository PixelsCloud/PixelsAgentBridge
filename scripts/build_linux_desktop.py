"""Build the Linux GUI on an Ubuntu 22.04 sysroot without changing headless builds."""
from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import json
import os
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
IMAGE = 'pab-linux-desktop-build-deps:jammy'
VOLUME = 'pab-linux-desktop-jammy-target'


def run(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def snapshot(destination):
    # Copy tracked source at its current working-tree state; do not include keys,
    # installed applications, node_modules, or shared build caches.
    files = subprocess.check_output(['git', 'ls-files', '-z'], cwd=ROOT).decode().split('\0')
    for name in files:
        if not name or not (ROOT / name).is_file():
            continue
        if name.startswith(('.env/', '.build/')):
            continue
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    # New Linux files are intentionally included before they are committed.
    for folder in ['apps/desktop/src-tauri/src', 'packaging/desktop/linux-desktop', 'vendor/libspa']:
        for source in (ROOT / folder).rglob('*'):
            if source.is_file():
                target = destination / source.relative_to(ROOT)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(source, target)
    shutil.copytree(ROOT / 'apps/desktop/dist', destination / 'apps/desktop/dist', dirs_exist_ok=True)


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True, help='Independent Linux desktop package version')
    parser.add_argument('--skip-image-build', action='store_true')
    parser.add_argument('--package', action='store_true')
    parser.add_argument('--headless-package', type=Path, help='Verified Linux headless archive to supply the stable backend')
    parser.add_argument('--headless-sha256', help='Expected SHA-256 of that headless archive')
    args = parser.parse_args()
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)', args.version):
        parser.error('Use a numeric version with minor/patch below 100')
    if args.package and not (args.headless_package and args.headless_sha256):
        parser.error('--package requires --headless-package and --headless-sha256')
    output = ROOT / '.build/linux-desktop' / args.version
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ, PAB_INSTALLER_VERSION=args.version)
    npm = shutil.which('npm.cmd' if os.name == 'nt' else 'npm')
    if not npm:
        parser.error('npm is required')
    run([npm, 'run', 'build:assets'], cwd=ROOT / 'apps/desktop', env=environment)
    source = output / 'source'
    source.mkdir(exist_ok=True)
    snapshot(source)
    if not args.skip_image_build:
        run(['docker', 'build', '--target', 'dependencies', '-t', IMAGE,
             '-f', ROOT / 'packaging/desktop/Dockerfile.linux-desktop', ROOT])
    cargo_cache = ROOT / '.build/linux-cargo-home'
    cargo_cache.mkdir(parents=True, exist_ok=True)
    script = '''set -eu
cargo build --locked --release --features tauri/custom-protocol --manifest-path apps/desktop/src-tauri/Cargo.toml -j 8
cp /target/release/pab-desktop /out/pab-desktop
'''
    run(['docker', 'run', '--rm', '--cpus', '10',
         # Tauri writes generated schemas next to its manifest. This is an
         # isolated source copy, never the user's working tree.
         '--mount', f'type=bind,source={source},target=/src',
         '--mount', f'type=bind,source={output},target=/out',
         '--mount', f'type=bind,source={cargo_cache},target=/cargo-home',
         '--mount', f'type=volume,source={VOLUME},target=/target',
         '--workdir', '/src', '--env', 'CARGO_HOME=/cargo-home',
         '--env', 'RUSTUP_TOOLCHAIN=1.95.0',
         '--env', 'CARGO_TARGET_DIR=/target', '--env', 'CARGO_BUILD_JOBS=8',
         '--env', 'CMAKE_BUILD_PARALLEL_LEVEL=2', '--env', 'CARGO_PROFILE_RELEASE_LTO=false',
         '--env', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16',
         '--env', f'PAB_INSTALLER_VERSION={args.version}', IMAGE, 'bash', '-c', script])
    binary = output / 'pab-desktop'
    metadata = {'version': args.version, 'platform': 'linux-desktop', 'architecture': 'x86_64',
                'sysroot': 'Ubuntu 22.04', 'cargo_jobs': 8, 'cmake_jobs': 2,
                'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT).decode().strip(),
                'sha256': sha256(binary.read_bytes()).hexdigest()}
    (output / 'build.json').write_text(json.dumps(metadata, indent=2) + '\n', encoding='utf-8')
    if args.package:
        run(['python', ROOT / 'packaging/desktop/build_linux_desktop_package.py',
             '--version', args.version, '--desktop', binary, '--headless-package', args.headless_package,
             '--headless-sha256', args.headless_sha256,
             '--output-dir', output])
    print('Linux desktop build: ' + str(output), flush=True)


if __name__ == '__main__':
    main()
