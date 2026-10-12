"""Create an offline, version-pinned browser runtime archive for the host OS.

The archive contains Node and @playwright/mcp for the Pixels-owned browser.
Target machines never run npm or contact Google to use the supported path.
"""
from argparse import ArgumentParser
from hashlib import sha256
from pathlib import Path
import gzip
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
from urllib.request import urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SOURCE = Path(__file__).resolve().parent
BUILD = ROOT / '.build' / 'browser-runtime'
VERSION = 'v22.15.0'
MCP_VERSION = '0.0.83'
NODE_RELEASE = 'https://nodejs.org/download/release/v22.15.0/'
# Hashes from Node's signed SHASUMS256.txt for this release.
NODE_SOURCES = {
    ('windows', 'x86_64'): ('win-x64/node.exe', '77bdff912b1c569b3e693fe126f619337c3e9d73dafbc4d0bf1d4f1f6a145761'),
    ('macos', 'arm64'): ('node-v22.15.0-darwin-arm64.tar.xz', '6e278a107d50da24b644dd26810a639a5f8ca67b55086e6b693caabcbb759912'),
    ('macos', 'x86_64'): ('node-v22.15.0-darwin-x64.tar.xz', '7dab3f93551d88f1e63db6b32bae6d4858e16740e9849ebbeac1d43f5055d8f0'),
    ('linux', 'x86_64'): ('node-v22.15.0-linux-x64.tar.xz', 'dafe2e8f82cb97de1bd10db9e2ec4c07bbf53389b0799b1e095a918951e78fd4'),
}


def digest(path):
    value = sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def pinned_node(platform_name, architecture, override):
    if (platform_name, architecture) not in NODE_SOURCES:
        raise ValueError(f'Unsupported Node target: {platform_name}/{architecture}')
    artifact_name, expected = NODE_SOURCES[(platform_name, architecture)]
    cache = BUILD / 'cache'
    cache.mkdir(parents=True, exist_ok=True)
    artifact = cache / Path(artifact_name).name
    if override:
        if platform_name != 'windows' or digest(override) != expected:
            raise ValueError('--node is only supported for the pinned Windows executable')
        checked = artifact
        if override.resolve() != checked.resolve():
            shutil.copy2(override, checked)
        return checked
    if not artifact.is_file() or digest(artifact) != expected:
        partial = artifact.with_name(artifact.name + '.part')
        with urlopen(NODE_RELEASE + artifact_name, timeout=60) as response, partial.open('wb') as output:
            shutil.copyfileobj(response, output)
        if digest(partial) != expected:
            partial.unlink(missing_ok=True)
            raise ValueError('Downloaded Node artifact did not match the official release SHA-256')
        partial.replace(artifact)
    if platform_name == 'windows':
        return artifact
    binary = cache / f'node-{platform_name}-{architecture}'
    partial = binary.with_name(binary.name + '.part')
    member_name = artifact_name.removesuffix('.tar.xz') + '/bin/node'
    with tarfile.open(artifact, 'r:xz') as source:
        member = source.getmember(member_name)
        with source.extractfile(member) as input_stream, partial.open('wb') as output:
            shutil.copyfileobj(input_stream, output)
    partial.chmod(0o755)
    partial.replace(binary)
    return binary


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument('--node', type=Path, help='already downloaded pinned Windows node.exe')
    parser.add_argument('--platform', choices=('windows', 'macos', 'linux'),
                        default={'Windows': 'windows', 'Darwin': 'macos', 'Linux': 'linux'}[platform.system()])
    args = parser.parse_args()
    architecture = {'AMD64':'x86_64', 'x86_64':'x86_64', 'arm64':'arm64', 'aarch64':'arm64'}.get(platform.machine(), platform.machine())
    if args.platform != {'Windows':'windows','Darwin':'macos','Linux':'linux'}[platform.system()]:
        parser.error('build the browser runtime on its target OS')
    if args.node and not args.node.is_file():
        parser.error('--node must name an existing file')
    node = pinned_node(args.platform, architecture, args.node)
    version = subprocess.check_output([str(node), '--version'], text=True).strip()
    if version != VERSION:
        parser.error(f'Node must be {VERSION}, got {version}')
    package = json.loads((SOURCE / 'package.json').read_text(encoding='utf-8'))
    if package['dependencies']['@playwright/mcp'] != MCP_VERSION:
        parser.error('Playwright MCP pin changed')
    installed_package = SOURCE / 'node_modules' / '@playwright' / 'mcp' / 'package.json'
    installed_version = json.loads(installed_package.read_text(encoding='utf-8')).get('version') if installed_package.is_file() else None
    if installed_version != MCP_VERSION or not (SOURCE / 'node_modules' / '@playwright' / 'mcp' / 'cli.js').is_file():
        env = dict(os.environ, PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD='1')
        subprocess.run(['npm', 'ci', '--ignore-scripts', '--no-audit', '--no-fund'],
                       cwd=SOURCE, env=env, check=True)
    BUILD.mkdir(parents=True, exist_ok=True)
    stage = BUILD / args.platform
    # The script only replaces its own .build/browser-runtime/<platform> staging tree.
    if not stage.resolve().is_relative_to(BUILD.resolve()):
        raise RuntimeError('browser runtime staging path escaped workspace')
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir()
    shutil.copy2(node, stage / ('node.exe' if args.platform == 'windows' else 'node'))
    shutil.copytree(SOURCE / 'node_modules', stage / 'node_modules')
    for name in ('NODE-LICENSE.txt', 'PLAYWRIGHT-LICENSE.txt'):
        shutil.copy2(SOURCE / name, stage / name)
    files = sorted(p for p in stage.rglob('*') if p.is_file())
    manifest = {
        'node': version,
        'playwright_mcp': MCP_VERSION,
        'files': {p.relative_to(stage).as_posix(): {'bytes': p.stat().st_size, 'sha256': digest(p)}
                  for p in files},
    }
    (stage / 'manifest.json').write_text(json.dumps(manifest, sort_keys=True, indent=2) + '\n', encoding='utf-8')
    if args.platform == 'windows':
        archive = BUILD / 'browser-runtime-windows.zip'
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as output:
            for path in sorted(stage.rglob('*')):
                if path.is_file():
                    info = zipfile.ZipInfo(path.relative_to(stage).as_posix(), (2020, 1, 1, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o644 << 16
                    with path.open('rb') as source, output.open(info, 'w') as target:
                        shutil.copyfileobj(source, target)
    else:
        archive = BUILD / f'browser-runtime-{args.platform}.tar.gz'
        with archive.open('wb') as raw, gzip.GzipFile(fileobj=raw, mode='wb', filename='', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w') as output:
                def reproducible(info):
                    info.mtime = info.uid = info.gid = 0
                    info.uname = info.gname = ''
                    return info
                output.add(stage, arcname='browser-runtime', filter=reproducible)
    print(json.dumps({'file': str(archive), 'bytes': archive.stat().st_size,
                      'sha256': digest(archive)}, indent=2))


if __name__ == '__main__':
    main()
