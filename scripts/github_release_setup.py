"""Prepare disposable GitHub-hosted release machines; never modify developer hosts."""
import argparse
import os
from pathlib import Path
import platform
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    subprocess.run(list(args), cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True)
    parser.add_argument('--product', choices=('client', 'management', 'relay'), required=True)
    args = parser.parse_args()
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('RUNNER_ENVIRONMENT') != 'github-hosted':
        raise ValueError('Tool installation is restricted to disposable GitHub-hosted runners')
    toolchain = tomllib.loads((ROOT / 'rust-toolchain.toml').read_text())['toolchain']['channel']
    run('rustup', 'toolchain', 'install', toolchain, '--profile', 'minimal')
    host = platform.system()
    if host == 'Windows':
        run('choco', 'install', 'nasm', 'nsis', '--yes', '--no-progress')
        with Path(os.environ['GITHUB_PATH']).open('a') as stream:
            stream.write('C:\\Program Files\\NASM\nC:\\Program Files (x86)\\NSIS\n')
    elif host == 'Darwin':
        run('brew', 'install', 'nasm', 'pkg-config', 'cmake')
    else:
        run('sudo', 'apt-get', 'update')
        run('sudo', 'apt-get', 'install', '-y', 'nasm', 'cmake', 'pkg-config', 'libssl-dev')
    if args.target == 'android':
        sdk = Path(os.environ['ANDROID_HOME'])
        manager = sdk / 'cmdline-tools/latest/bin/sdkmanager'
        run(str(manager), 'platforms;android-36', 'build-tools;36.0.0', 'ndk;29.0.14206865')
        run('rustup', 'target', 'add', '--toolchain', toolchain, 'aarch64-linux-android', 'x86_64-linux-android')
    elif args.target.startswith('macos-'):
        arch = 'aarch64' if args.target == 'macos-arm64' else 'x86_64'
        run('rustup', 'target', 'add', '--toolchain', toolchain, arch + '-apple-darwin')


if __name__ == '__main__':
    main()
