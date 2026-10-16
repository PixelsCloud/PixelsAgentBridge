"""Package the native Linux GUI and a verified, unchanged headless backend as a deb."""
from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import json
import re
import shutil
import subprocess
import tarfile
import uuid

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = Path(__file__).resolve().parent / 'linux-desktop'
ALLOWED = {'pab-executor','pab-mcp','pab-document-worker','document-runtime-linux.tar.gz',
           'install.sh','uninstall.sh','lifecycle.sh','run-mcp.sh','run-executor.sh','INSTALL-LINUX.txt','release.json'}


def verify_desktop(binary, version):
    with binary.open('rb') as stream:
        header = stream.read(20)
        digest = sha256(header)
        while chunk := stream.read(1024 * 1024): digest.update(chunk)
    if header[:6] != b'\x7fELF\x02\x01' or header[18:20] != b'\x3e\x00':
        raise ValueError('Desktop executable must be an x86_64 ELF')
    receipt = json.loads(binary.with_name('build.json').read_text(encoding='utf-8'))
    if (receipt.get('version'), receipt.get('platform'), receipt.get('architecture'), receipt.get('sha256')) != (
        version, 'linux-desktop', 'x86_64', digest.hexdigest()
    ):
        raise ValueError('Desktop build receipt does not match the version, platform or executable')


def unpack_backend(archive, expected_sha256, destination):
    if not re.fullmatch('[0-9a-f]{64}', expected_sha256):
        raise ValueError('Expected backend checksum must be SHA-256')
    with archive.open('rb') as stream:
        digest = sha256()
        while chunk := stream.read(1024 * 1024): digest.update(chunk)
    if digest.hexdigest() != expected_sha256:
        raise ValueError('Backend archive checksum does not match')
    with tarfile.open(archive, 'r:gz') as tar:
        members = tar.getmembers()
        # Accept verified older headless archives too; this Desktop build adds
        # its own current notice below, independent of the backend version.
        names = {m.name for m in members}
        if len(members) != len(names) or names not in (ALLOWED, ALLOWED | {'THIRD-PARTY-NOTICES.txt'}):
            raise ValueError('Unexpected backend package entries')
        if any(not m.isfile() or not 0 <= m.size <= 500_000_000 for m in members):
            raise ValueError('Unsafe backend archive entry')
        if sum(m.size for m in members) > 1_000_000_000:
            raise ValueError('Backend package exceeds extraction budget')
        for member in members:
            if member.name in {'install.sh','uninstall.sh','document-runtime-linux.tar.gz'}: continue
            target = destination / member.name
            with tar.extractfile(member) as source, target.open('wb') as output: shutil.copyfileobj(source, output)
        with tar.extractfile('document-runtime-linux.tar.gz') as stream, tarfile.open(fileobj=stream, mode='r|gz') as runtime:
            count = total = 0
            names = set()
            for member in runtime:
                count += 1; total += member.size
                path = Path(member.name)
                if (path.is_absolute() or '..' in path.parts or '\\' in member.name
                    or not path.parts or path.parts[0] != 'document-runtime'
                    or not (member.isfile() or member.isdir()) or member.name in names
                    or count > 100 or total > 600_000_000):
                    raise ValueError('Unsafe document runtime entry')
                names.add(member.name)
                target = destination / path
                if member.isdir(): target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with runtime.extractfile(member) as source, target.open('wb') as output: shutil.copyfileobj(source, output)
    metadata = json.loads((destination / 'release.json').read_text())
    if (metadata.get('platform'), metadata.get('architecture')) != ('linux', 'x86_64'):
        raise ValueError('Backend platform does not match Linux x86_64')
    if not (destination / 'document-runtime/libpdfium.so').is_file():
        raise ValueError('PDF runtime is missing')
    return metadata


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('--version', required=True)
    parser.add_argument('--desktop', type=Path, required=True)
    parser.add_argument('--headless-package', type=Path, required=True)
    parser.add_argument('--headless-sha256', required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)', args.version):
        parser.error('Invalid desktop package version')
    verify_desktop(args.desktop, args.version)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    stage = args.output_dir / ('deb-root-' + uuid.uuid4().hex)
    install = stage / 'opt/pixels-agent-bridge'
    install.mkdir(parents=True)
    backend = unpack_backend(args.headless_package, args.headless_sha256, install)
    shutil.copy2(args.desktop, install / 'pab-desktop')
    shutil.copy2(ROOT / 'packaging/desktop/windows/THIRD-PARTY-NOTICES.txt', install / 'THIRD-PARTY-NOTICES.txt')
    for name in ['run-desktop.sh','grant-desktop-user.sh']: shutil.copy2(SCRIPTS / name, install / name)
    (install / 'desktop-release.toml').write_text(
        f'version = "{args.version}"\nbackend_version = "{backend["version"]}"\n'
        f'backend_sha256 = "{args.headless_sha256}"\n', encoding='utf-8')
    entries = {
        'usr/share/applications/pixels-agent-bridge.desktop': SCRIPTS / 'pixels-agent-bridge.desktop',
        'etc/xdg/autostart/pixels-agent-bridge.desktop': SCRIPTS / 'pixels-agent-bridge-autostart.desktop',
        'lib/systemd/system/pixels-agent-bridge-executor.service': SCRIPTS / 'pixels-agent-bridge-executor.service',
        'usr/share/icons/hicolor/128x128/apps/pixels-agent-bridge.png': ROOT / 'apps/desktop/src-tauri/icons/128x128.png',
    }
    for name, source in entries.items():
        destination = stage / name; destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    command = stage / 'usr/bin/pixels-agent-bridge'
    command.parent.mkdir(parents=True, exist_ok=True)
    command.write_text('#!/bin/sh\nexec /opt/pixels-agent-bridge/run-desktop.sh "$@"\n', encoding='utf-8')
    control = stage / 'DEBIAN'; control.mkdir()
    (control / 'control').write_text(
        f'Package: pixels-agent-bridge\nVersion: {args.version}\nArchitecture: amd64\n'
        'Maintainer: Pixels <support@rgaa.vip>\nSection: net\nPriority: optional\n'
        'Depends: libc6 (>= 2.35), libstdc++6, libgcc-s1, libgtk-3-0, libwebkit2gtk-4.1-0, '
        'libayatana-appindicator3-1, libxdo3, libpipewire-0.3-0, libgbm1, libegl1, '
        'libwayland-client0, libwayland-server0, libxcb1, systemd, util-linux, bash\n'
        'Description: Pixels Agent Bridge desktop and device service\n'
        ' Native graphical client, remote terminal, MCP and offline document tools.\n', encoding='utf-8')
    for name in ['postinst','prerm','postrm']: shutil.copy2(SCRIPTS / name, control / name)
    # Git on Windows may check out CRLF. All installed launch/maintainer scripts are LF.
    for path in stage.rglob('*'):
        if path.is_file() and (path == command or path.suffix in ('.sh','.desktop','.service') or path.parent == control):
            path.write_bytes(path.read_bytes().replace(b'\r\n',b'\n'))
    filename = f'pixels-agent-bridge-desktop-linux-x86_64-{args.version}.deb'
    command_text = '''set -eu
find /package -type d -exec chmod 755 {} +
find /package -type f -exec chmod 644 {} +
chmod 755 /package/DEBIAN/postinst /package/DEBIAN/prerm /package/DEBIAN/postrm
chmod 755 /package/opt/pixels-agent-bridge/pab-* /package/opt/pixels-agent-bridge/*.sh /package/usr/bin/pixels-agent-bridge
dpkg-deb --root-owner-group -Zgzip -z6 --build /package /out/''' + filename
    subprocess.run(['docker','run','--rm','--cpus','2','--mount',f'type=bind,source={stage.resolve()},target=/package',
        '--mount',f'type=bind,source={args.output_dir.resolve()},target=/out',
        'pab-linux-desktop-build-deps:jammy','bash','-c',command_text],check=True)
    package = args.output_dir / filename
    result = {'version':args.version,'platform':'linux-desktop','architecture':'x86_64','filename':filename,
              'bytes':package.stat().st_size,'sha256':sha256(package.read_bytes()).hexdigest(),
              'backend_version':backend['version'],'backend_sha256':args.headless_sha256,
              'desktop_sha256':sha256(args.desktop.read_bytes()).hexdigest()}
    (args.output_dir / 'package.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
    # Delete only this invocation's verified staging directory, never its output
    # parent, source checkout, or another build's directory.
    assert stage.resolve().parent == args.output_dir.resolve()
    assert stage.name.startswith('deb-root-') and not stage.is_symlink()
    shutil.rmtree(stage)
    print(json.dumps(result,indent=2))


if __name__ == '__main__': main()
