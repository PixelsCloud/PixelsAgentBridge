"""Validate Actions syntax with a checksum-pinned actionlint, without compiling tools."""
import hashlib
import io
import os
from pathlib import Path
import platform
import subprocess
import tarfile
from urllib.request import urlopen
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VERSION = '1.7.7'
ASSETS = {
    ('Windows', 'AMD64'): ('windows_amd64.zip', '7f12f1801bca3d480d67aaf7774f4c2a6359a3ca8eebe382c95c10c9704aa731', 'actionlint.exe'),
    ('Linux', 'x86_64'): ('linux_amd64.tar.gz', '023070a287cd8cccd71515fedc843f1985bf96c436b7effaecce67290e7e0757', 'actionlint'),
}


def main():
    suffix, expected, executable = ASSETS[(platform.system(), platform.machine())]
    url = f'https://github.com/rhysd/actionlint/releases/download/v{VERSION}/actionlint_{VERSION}_{suffix}'
    # Verify the archive on every invocation; a cache is not a trust source.
    folder = ROOT / '.build/github-release/tools'
    folder.mkdir(parents=True, exist_ok=True)
    cached = folder / ('actionlint_' + VERSION + '_' + suffix)
    data = cached.read_bytes() if cached.is_file() else b''
    if hashlib.sha256(data).hexdigest() != expected:
        with urlopen(url, timeout=60) as response:
            data = response.read(20 * 1024 * 1024 + 1)
        if len(data) > 20 * 1024 * 1024 or hashlib.sha256(data).hexdigest() != expected:
            raise ValueError('actionlint archive checksum mismatch')
        cached.write_bytes(data)
    if suffix.endswith('.zip'):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            binary = archive.read(executable)
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
            member = archive.getmember(executable)
            if not member.isfile():
                raise ValueError('Unexpected actionlint archive entry')
            binary = archive.extractfile(member).read()
    tool = folder / executable
    tool.write_bytes(binary)
    if os.name != 'nt':
        tool.chmod(0o755)
    return subprocess.run([str(tool), '-shellcheck=', '-pyflakes='], cwd=ROOT).returncode


if __name__ == '__main__':
    raise SystemExit(main())
