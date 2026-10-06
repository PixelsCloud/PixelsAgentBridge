"""Build product components with one shared, automatically incremented version."""
from argparse import ArgumentParser
from pathlib import Path
import os
import platform
import shutil
import subprocess
import sys

from build_version import build_lock, reserve_version, record_artifacts, macos_artifacts, macos_app_version

ROOT = Path(__file__).resolve().parents[1]


def run(command, cwd=ROOT):
    command = [str(part) for part in command]
    print('> ' + ' '.join(command), flush=True)
    subprocess.run(command, cwd=cwd, check=True)


def web_files():
    folder = ROOT / 'apps/web/dist'
    return {'web/' + path.relative_to(folder).as_posix(): path for path in sorted(folder.rglob('*')) if path.is_file()}


def macos_architectures(selection):
    if selection == 'all':
        return ['aarch64', 'x86_64']
    if selection == 'native':
        selection = platform.machine()
    if selection == 'arm64':
        selection = 'aarch64'
    if selection not in ('aarch64', 'x86_64'):
        raise ValueError(f'Unsupported macOS architecture: {selection}')
    return [selection]


def build_macos(version, profile, architectures, package):
    from macos_signing import sign, IDENTIFIERS
    for architecture in architectures:
        target = f'{architecture}-apple-darwin'
        flags = ['--release'] if profile == 'release' else []
        run(['cargo', 'build', '--locked', '--target', target, *flags,
             '-p', 'pab-executor', '--bin', 'pab-executor', '-p', 'pab-bridge', '--bin', 'pab-mcp'])
        run(['node', 'node_modules/@tauri-apps/cli/tauri.js', 'build', '--target', target,
             '--bundles', 'app', *(['--debug'] if profile == 'debug' else [])], ROOT / 'apps/desktop')
        binaries = ROOT / 'target' / target / profile
        app = ROOT / 'apps/desktop/src-tauri/target' / target / profile / 'bundle/macos/Pixels Agent Bridge.app'
        if macos_app_version(app) != version:
            raise ValueError(f'Built macOS app version does not match {version}')
        for name in ('executor', 'mcp'):
            sign(binaries / f'pab-{name}', IDENTIFIERS[name])
        sign(app, IDENTIFIERS['desktop'])
        record_artifacts(ROOT, f'macos-{architecture}', profile, version, macos_artifacts(binaries, app))
        if package:
            run([sys.executable, 'packaging/desktop/build.py', '--platform', 'macos', '--profile', profile,
                 '--macos-arch', architecture, '--macos-bin-dir', binaries, '--macos-app', app])
            run([sys.executable, 'packaging/desktop/build_macos_pkg.py', '--arch', architecture, '--profile', profile])


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('targets', nargs='+', choices=['desktop', 'server', 'web', 'desktop-web', 'docker', 'linux', 'macos'])
    parser.add_argument('--profile', choices=['debug', 'release'], default='debug')
    parser.add_argument('--package', action='store_true', help='Also package desktop/server artifacts; does not increment again')
    parser.add_argument('--image', help='Docker image tag (default: pixels-agent-bridge:<version>)')
    parser.add_argument('--macos-arch', choices=['native', 'arm64', 'aarch64', 'x86_64', 'all'], default=None)
    args = parser.parse_args()
    targets = list(dict.fromkeys(args.targets))
    architectures = []
    if args.macos_arch and 'macos' not in targets:
        parser.error('--macos-arch requires the macos target')
    if 'macos' in targets:
        if sys.platform != 'darwin':
            parser.error('Build the macos target on a Mac')
        if args.package and sys.version_info < (3, 12):
            parser.error('macOS PKG packaging requires Python 3.12 or newer')
        architectures = macos_architectures(args.macos_arch or 'native')
    if 'desktop' in targets and os.name != 'nt':
        parser.error('desktop builds the Windows installer components; use linux for Linux Docker builds')
    if 'linux' in targets and os.name != 'nt':
        parser.error('linux currently uses the Windows-host Docker build script')
    if args.image and 'docker' not in targets:
        parser.error('--image requires the docker target')
    if 'docker' in targets and args.profile != 'release':
        parser.error('docker uses release binaries; specify --profile release explicitly')
    needs_node = any(t not in ('docker', 'linux') for t in targets)
    required = (['npm'] if needs_node else []) + (['cargo'] if any(t in targets for t in ['desktop', 'server', 'macos']) else []) + (['docker'] if any(t in targets for t in ['docker', 'linux']) else [])
    if 'macos' in targets:
        required += ['node', 'rustup'] + (['/usr/bin/pkgbuild', '/usr/bin/productbuild'] if args.package else [])
    for tool in required:
        if not shutil.which(tool):
            parser.error(f'{tool} is not installed')
    if any(t in targets for t in ['desktop', 'macos']) and not (ROOT / 'apps/desktop/node_modules/@tauri-apps/cli/tauri.js').is_file():
        parser.error('Run npm ci in apps/desktop first')
    if architectures:
        from macos_signing import load_identity
        load_identity()  # Fail before allocating a version; never silently regenerate keys.
        installed = subprocess.check_output(['rustup', 'target', 'list', '--installed'], cwd=ROOT, text=True).splitlines()
        missing = [f'{arch}-apple-darwin' for arch in architectures if f'{arch}-apple-darwin' not in installed]
        if missing:
            parser.error('Install required targets first: rustup target add ' + ' '.join(missing))
    with build_lock(ROOT):
        version = reserve_version(ROOT)
        print(f'Building Pixels Agent Bridge {version}', flush=True)
        npm = shutil.which('npm')
        profile_flags = ['--release'] if args.profile == 'release' else []
        built_frontends = set()

        def frontend(app):
            if app not in built_frontends:
                run([npm, 'run', 'build:assets'], ROOT / 'apps' / app)
                built_frontends.add(app)

        for target in targets:
            if target in ('web', 'desktop-web'):
                frontend('web' if target == 'web' else 'desktop')
            elif target == 'macos':
                build_macos(version, args.profile, architectures, args.package)
                built_frontends.add('desktop')
            elif target == 'desktop':
                run(['cargo', 'build', '--locked', *profile_flags, '-p', 'pab-executor', '--bin', 'pab-executor', '-p', 'pab-bridge', '--bin', 'pab-mcp'])
                run(['node', 'node_modules/@tauri-apps/cli/tauri.js', 'build', '--no-bundle', *(['--debug'] if args.profile == 'debug' else [])], ROOT / 'apps/desktop')
                built_frontends.add('desktop')
                files = {name: ROOT / 'target' / args.profile / name for name in ['pab-executor.exe', 'pab-mcp.exe']}
                files['pab-desktop.exe'] = ROOT / 'apps/desktop/src-tauri/target' / args.profile / 'pab-desktop.exe'
                record_artifacts(ROOT, target, args.profile, version, files)
                if args.package:
                    run([sys.executable, 'packaging/desktop/build.py', '--platform', 'windows', '--profile', args.profile])
                    run([sys.executable, 'packaging/desktop/build_nsis.py', '--profile', args.profile])
            elif target == 'server':
                frontend('web')
                run(['cargo', 'build', '--locked', *profile_flags, '-p', 'pab-server', '--bin', 'pab-server', '-p', 'pab-relay', '--bin', 'pab-relay-server'])
                suffix = '.exe' if os.name == 'nt' else ''
                files = {name + suffix: ROOT / 'target' / args.profile / (name + suffix) for name in ['pab-server', 'pab-relay-server']}
                record_artifacts(ROOT, target, args.profile, version, files | web_files())
                if args.package:
                    run([sys.executable, 'packaging/server/build.py', '--platform', 'windows' if os.name == 'nt' else 'linux', '--profile', args.profile])
            elif target == 'docker':
                run(['docker', 'build', '-f', 'packaging/docker/Dockerfile', '-t', args.image or f'pixels-agent-bridge:{version}', '.'])
            elif target == 'linux':
                run(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'packaging/desktop/build-linux.ps1', '-Profile', args.profile, '-ComponentsOnly'])
                files = {name: ROOT / f'.build/guest-desktop-linux-{args.profile}' / name for name in ['pab-executor', 'pab-mcp']}
                record_artifacts(ROOT, target, args.profile, version, files)
                if args.package:
                    run([sys.executable, 'packaging/desktop/build.py', '--platform', 'linux', '--profile', args.profile])
        print(f'Build complete: {version}', flush=True)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Build failed: {error}', file=sys.stderr)
        sys.exit(1)
