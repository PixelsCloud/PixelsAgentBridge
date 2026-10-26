"""Build components with stable internal versions and an incrementing installer version."""
from argparse import ArgumentParser
from pathlib import Path
import os
import platform
import shutil
import subprocess
import sys

from build_version import build_lock, reserve_product_version, product_for_target, record_artifacts, macos_artifacts, macos_app_version, parse_version
from service_products import BINARIES, artifacts as service_artifacts

ROOT = Path(__file__).resolve().parents[1]
CARGO_TARGET = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target')).resolve()
# Desktop and workspace builds share one persistent cache in the main checkout.
TAURI_TARGET = CARGO_TARGET
os.environ['CARGO_TARGET_DIR'] = str(CARGO_TARGET)
CARGO_JOBS = max(1, min(10, int(os.environ.get('CARGO_BUILD_JOBS', '10'))))
os.environ['CARGO_BUILD_JOBS'] = str(CARGO_JOBS)
os.environ['CMAKE_BUILD_PARALLEL_LEVEL'] = '1'


def run(command, cwd=ROOT):
    command = [str(part) for part in command]
    print('> ' + ' '.join(command), flush=True)
    subprocess.run(command, cwd=cwd, check=True)


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


def build_macos(version, profile, architectures, package, installer_identity=None):
    from macos_signing import sign, IDENTIFIERS
    for architecture in architectures:
        if package:
            run([sys.executable, 'browser-runtime/build.py', '--arch', architecture])
        target = f'{architecture}-apple-darwin'
        flags = ['--release'] if profile == 'release' else []
        run(['cargo', 'build', '--locked', '--target', target, *flags,
             '-p', 'pab-executor', '--bin', 'pab-executor', '-p', 'pab-bridge', '--bin', 'pab-mcp',
             '-p', 'pab-document', '--bin', 'pab-document-worker', '-j', str(CARGO_JOBS)])
        run(['node', 'node_modules/@tauri-apps/cli/tauri.js', 'build', '--target', target,
             '--bundles', 'app', *(['--debug'] if profile == 'debug' else []), '--', '--locked'], ROOT / 'apps/desktop')
        binaries = CARGO_TARGET / target / profile
        app = TAURI_TARGET / target / profile / 'bundle/macos/Pixels Agent Bridge.app'
        macos_app_version(app)  # Validate the independent, stable app version.
        for name in ('executor', 'mcp'):
            sign(binaries / f'pab-{name}', IDENTIFIERS[name])
        sign(binaries / 'pab-document-worker', IDENTIFIERS['document'])
        sign(app, IDENTIFIERS['desktop'])
        record_artifacts(ROOT, f'macos-{architecture}', profile, version, macos_artifacts(binaries, app))
        if package:
            run([sys.executable, 'packaging/desktop/document_runtime.py', '--platform',
                 'macos-' + ('arm64' if architecture == 'aarch64' else 'x64')])
            run([sys.executable, 'packaging/desktop/build.py', '--platform', 'macos', '--profile', profile,
                 '--macos-arch', architecture, '--macos-bin-dir', binaries, '--macos-app', app])
            command = [sys.executable, 'packaging/desktop/build_macos_pkg.py', '--arch', architecture, '--profile', profile]
            if installer_identity:
                command += ['--sign', installer_identity]
            run(command)


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('targets', nargs='+', choices=['desktop', 'management', 'relay', 'server', 'web', 'desktop-web', 'docker-management', 'docker-relay', 'docker', 'linux', 'macos', 'dsh-pab'])
    parser.add_argument('--profile', choices=['debug', 'release'], default='debug')
    parser.add_argument('--package', action='store_true', help='Also package desktop/service artifacts; does not increment again')
    parser.add_argument('--image', help='Override the image tag for one Docker target')
    parser.add_argument('--macos-arch', choices=['native', 'arm64', 'aarch64', 'x86_64', 'all'], default=None)
    parser.add_argument('--installer-version', help='Version reserved by the release service; does not change local build-version.json')
    parser.add_argument('--release-version', help='Explicit CI product version; never increments local counters')
    parser.add_argument('--macos-installer-identity', help='Developer ID Installer identity for the macOS PKG')
    args = parser.parse_args()
    if args.release_version:
        parse_version(args.release_version)
        products = {product_for_target(target) for target in args.targets}
        if args.installer_version or len(products) != 1 or args.profile != 'release' or not args.package:
            parser.error('--release-version requires one product, release profile and --package')
    if args.installer_version:
        parse_version(args.installer_version)
        if len(args.targets) != 1 or args.targets[0] not in ('desktop', 'macos', 'linux') or args.profile != 'release' or not args.package:
            parser.error('--installer-version requires one release desktop/macos/linux target with --package')
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
    docker_targets = [t for t in targets if t in ('docker', 'docker-management', 'docker-relay')]
    if args.image and len(docker_targets) != 1:
        parser.error('--image requires exactly one Docker target')
    if docker_targets and args.profile != 'release':
        parser.error('Docker targets use release binaries; specify --profile release explicitly')
    needs_node = any(t in ('desktop', 'management', 'server', 'web', 'desktop-web', 'macos', 'dsh-pab') for t in targets)
    required = (['npm'] if needs_node else []) + (['cargo'] if any(t in targets for t in ['desktop', 'management', 'relay', 'server', 'macos', 'dsh-pab']) else []) + (['docker'] if docker_targets or 'linux' in targets else [])
    if 'macos' in targets:
        required += ['node', 'rustup'] + (['/usr/bin/pkgbuild', '/usr/bin/productbuild'] if args.package else [])
    if 'desktop' in targets or ('macos' in targets and 'x86_64' in architectures):
        required.append('nasm')  # Bundled software H.264 needs its x86 SIMD build.
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
        versions = {}
        for target in targets:
            product = product_for_target(target)
            if product not in versions:
                versions[product] = args.release_version or args.installer_version or reserve_product_version(ROOT, product)
        source_before = None
        if (ROOT / '.git').exists() and any(t in BINARIES or t in ('docker-management', 'docker-relay') for t in targets):
            sys.path.insert(0, str(ROOT))
            from smoke.run import source_state
            source_before = source_state()
        print('Building ' + ', '.join(f'{p} {v}' for p, v in versions.items()), flush=True)
        print('Only product release counters advance; Rust, npm and Tauri versions stay unchanged.', flush=True)
        print('Cargo reuses cached artifacts and rebuilds only changed inputs.', flush=True)
        npm = shutil.which('npm')
        profile_flags = ['--release'] if args.profile == 'release' else []
        built_frontends = set()

        def frontend(app):
            if app not in built_frontends:
                run([npm, 'run', 'build:assets'], ROOT / 'apps' / app)
                built_frontends.add(app)

        for target in targets:
            version = versions[product_for_target(target)]
            os.environ['PAB_INSTALLER_VERSION'] = version
            if target in ('web', 'desktop-web'):
                frontend('web' if target == 'web' else 'desktop')
            elif target == 'macos':
                build_macos(version, args.profile, architectures, args.package, args.macos_installer_identity)
                built_frontends.add('desktop')
            elif target == 'desktop':
                if args.package:
                    run([sys.executable, 'browser-runtime/build.py'])
                    run([sys.executable, 'packaging/desktop/document_runtime.py', '--platform', 'windows'])
                run(['cargo', 'build', '--locked', *profile_flags, '-p', 'pab-executor', '--bin', 'pab-executor', '-p', 'pab-bridge', '--bin', 'pab-mcp',
                     '-p', 'pab-document', '--bin', 'pab-document-worker', '-j', str(CARGO_JOBS)])
                run(['node', 'node_modules/@tauri-apps/cli/tauri.js', 'build', '--no-bundle', *(['--debug'] if args.profile == 'debug' else []), '--', '--locked'], ROOT / 'apps/desktop')
                built_frontends.add('desktop')
                files = {name: CARGO_TARGET / args.profile / name for name in ['pab-executor.exe', 'pab-mcp.exe', 'pab-document-worker.exe']}
                files['pab-desktop.exe'] = TAURI_TARGET / args.profile / 'pab-desktop.exe'
                record_artifacts(ROOT, target, args.profile, version, files)
                if args.package:
                    run([sys.executable, 'packaging/desktop/build.py', '--platform', 'windows', '--profile', args.profile,
                         '--windows-bin-dir', CARGO_TARGET / args.profile, '--windows-desktop-bin', files['pab-desktop.exe']])
                    run([sys.executable, 'packaging/desktop/build_nsis.py', '--profile', args.profile])
            elif target == 'dsh-pab':
                # Release packaging: build pab-mcp and fold it into a self-contained
                # plugin directory plus npm tarball under .build/packages/.
                run([sys.executable, 'scripts/package_dsh_plugin.py', '--profile', args.profile, '--build'])
            elif target in BINARIES:
                if target in ('management', 'server'):
                    frontend('web')
                command = ['cargo', 'build', '--locked', *profile_flags, '-j', str(CARGO_JOBS)]
                if target in ('management', 'server'):
                    command += ['-p', 'pab-server', '--bin', 'pab-server']
                if target in ('relay', 'server'):
                    command += ['-p', 'pab-relay', '--bin', 'pab-relay-server', '--bin', 'pab-relay-healthcheck']
                run(command)
                service_platform = 'windows' if os.name == 'nt' else 'linux'
                files = service_artifacts(ROOT, target, service_platform, CARGO_TARGET / args.profile)
                if source_before is not None and source_state() != source_before:
                    raise RuntimeError('Sources changed during service build; rebuild before packaging')
                record_artifacts(ROOT, target, args.profile, version, files, source=source_before)
                if args.package:
                    run([sys.executable, 'packaging/server/build.py', '--product', target, '--platform', service_platform,
                         '--profile', args.profile, '--binary-dir', CARGO_TARGET / args.profile])
            elif target in docker_targets:
                product = target.removeprefix('docker-') if target != 'docker' else None
                command = ['docker', 'build', '-f', 'packaging/docker/Dockerfile']
                if product:
                    command += ['--target', product]
                    command += ['--build-arg', f'PAB_RELEASE_VERSION={version}']
                    if source_before is not None:
                        command += ['--build-arg', f'PAB_RELEASE_COMMIT={source_before["commit"]}',
                                    '--build-arg', f'PAB_SOURCE_SHA256={source_before["sha256"]}']
                repository = 'pixels-agent-bridge' + ('-' + product if product else '')
                image = args.image or f'{repository}:{version}'
                run([*command, '-t', image, '.'])
                if product and args.package:
                    if source_before is not None and source_state() != source_before:
                        raise RuntimeError('Sources changed during image build; rebuild before exporting')
                    from package_service_image import package_image
                    print(package_image(ROOT, product, version, image, source_before), flush=True)
            elif target == 'linux':
                if os.name == 'nt':
                    run(['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'packaging/desktop/build-linux.ps1', '-Profile', args.profile, '-ComponentsOnly'])
                else:
                    run([sys.executable, 'scripts/build_linux_headless.py', '--profile', args.profile])
                files = {name: ROOT / f'.build/guest-desktop-linux-{args.profile}' / name for name in ['pab-executor', 'pab-mcp', 'pab-document-worker']}
                record_artifacts(ROOT, target, args.profile, version, files)
                if args.package:
                    run([sys.executable, 'packaging/desktop/document_runtime.py', '--platform', 'linux'])
                    run([sys.executable, 'packaging/desktop/build.py', '--platform', 'linux', '--profile', args.profile])
        print('Build complete: ' + ', '.join(f'{p} {v}' for p, v in versions.items()), flush=True)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Build failed: {error}', file=sys.stderr)
        sys.exit(1)
