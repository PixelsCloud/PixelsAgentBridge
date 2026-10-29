"""Build the Linux GUI on an Ubuntu 22.04 sysroot without changing headless builds."""
from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import json
import os
import re
import shutil
import subprocess
from build_linux_headless import defaults_arguments as container_defaults, release_profile_arguments

ROOT = Path(__file__).resolve().parents[1]
IMAGE = 'pab-linux-desktop-build-deps:jammy'
VOLUME = 'pab-linux-desktop-jammy-target'
JOBS = max(1, min(10, int(os.environ.get('CARGO_BUILD_JOBS', '10'))))


def run(args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


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
    # Build the shared working tree directly. Wait for other edits/builds before
    # invoking this helper; never create a second checkout to bypass them.
    source = ROOT
    if not args.skip_image_build:
        run(['docker', 'build', '--target', 'dependencies', '-t', IMAGE,
             '-f', ROOT / 'packaging/desktop/Dockerfile.linux-desktop', ROOT])
    cargo_cache = ROOT / '.build/linux-cargo-home'
    cargo_cache.mkdir(parents=True, exist_ok=True)
    script = f'''set -eu
# Let Linux Makefile dependencies share Cargo's jobserver instead of forcing -j1.
unset CMAKE_BUILD_PARALLEL_LEVEL
cargo build --locked --release --features tauri/custom-protocol --manifest-path apps/desktop/src-tauri/Cargo.toml -j {JOBS}
cp /target/release/pab-desktop /out/pab-desktop
'''
    run(['docker', 'run', '--rm', '--cpus', str(JOBS),
         # Keep the source path stable across releases; only outputs are versioned.
         '--mount', f'type=bind,source={source},target=/src',
         '--mount', f'type=bind,source={output},target=/out',
         '--mount', f'type=bind,source={cargo_cache},target=/cargo-home',
         '--mount', f'type=volume,source={VOLUME},target=/target',
         '--workdir', '/src', '--env', 'CARGO_HOME=/cargo-home',
         '--env', 'RUSTUP_TOOLCHAIN=1.95.0',
         '--env', 'CARGO_TARGET_DIR=/target', '--env', f'CARGO_BUILD_JOBS={JOBS}',
         '--env', 'CARGO_PROFILE_RELEASE_LTO=false',
         '--env', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16',
         *release_profile_arguments(),
         '--env', f'PAB_INSTALLER_VERSION={args.version}',
         *container_defaults(), IMAGE, 'bash', '-c', script])
    binary = output / 'pab-desktop'
    metadata = {'version': args.version, 'platform': 'linux-desktop', 'architecture': 'x86_64',
                'sysroot': 'Ubuntu 22.04', 'cargo_jobs': JOBS, 'cmake_jobs': JOBS,
                'native_jobs_shared_with_cargo': True,
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
