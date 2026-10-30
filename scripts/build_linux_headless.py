"""Build the existing Bullseye headless components on either Windows or Linux."""
import argparse
import os
from pathlib import Path
import subprocess
from build_profile import apply_release_preset

ROOT = Path(__file__).resolve().parents[1]


def command(profile, jobs):
    if profile != 'release' or not 1 <= jobs <= 10:
        raise ValueError('Invalid profile or build budget')
    flags = '--release' if profile == 'release' else ''
    return f'''set -eu
unset CMAKE_BUILD_PARALLEL_LEVEL
cargo build --locked {flags} -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp -p pab-document --bin pab-document-worker -j {jobs}
mkdir -p .build/guest-desktop-linux-{profile}
cp .build/linux-target-bullseye/{profile}/pab-executor .build/linux-target-bullseye/{profile}/pab-mcp .build/linux-target-bullseye/{profile}/pab-document-worker .build/guest-desktop-linux-{profile}/
'''


def release_profile_arguments():
    return [argument for name in ('OPT_LEVEL', 'LTO', 'CODEGEN_UNITS', 'INCREMENTAL')
            if (value := os.environ.get('CARGO_PROFILE_RELEASE_' + name)) is not None
            for argument in ('--env', 'CARGO_PROFILE_RELEASE_' + name + '=' + value)]


def defaults_arguments():
    value = os.environ.get('PAB_CLIENT_DEFAULTS_FILE')
    if value is None:
        return []
    path = Path(value).resolve(strict=True)
    if not value or not path.is_relative_to(ROOT):
        raise ValueError('Container defaults must be under the mounted repository')
    return ['--env', 'PAB_CLIENT_DEFAULTS_FILE=/src/' + path.relative_to(ROOT).as_posix()]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('release',), default='release')
    parser.add_argument('--preset', choices=('fast', 'optimized'))
    args = parser.parse_args()
    apply_release_preset(os.environ, args.preset)
    jobs = max(1, min(10, int(os.environ.get('CARGO_BUILD_JOBS', '10'))))
    image = 'pab-linux-headless-build-deps:bullseye'
    subprocess.run(['docker', 'build', '--target', 'dependencies', '-t', image, '-f',
                    str(ROOT / 'packaging/desktop/Dockerfile.linux'), str(ROOT)], check=True)
    subprocess.run(['docker', 'run', '--rm', '--cpus', str(jobs), '--mount',
                    f'type=bind,source={ROOT},target=/src', '--workdir', '/src',
                    '--env', 'CARGO_HOME=/src/.build/linux-cargo-home',
                    '--env', 'CARGO_TARGET_DIR=/src/.build/linux-target-bullseye',
                    '--env', f'CARGO_INCREMENTAL={os.environ["CARGO_INCREMENTAL"]}', '--env', f'CARGO_BUILD_JOBS={jobs}',
                    '--env', 'PATH=/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
                    *release_profile_arguments(), *defaults_arguments(), image, 'sh', '-c', command(args.profile, jobs)], check=True)


if __name__ == '__main__':
    main()
