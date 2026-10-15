"""Exercise signed downloads and installation ordering without modifying a host."""
import base64
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'packaging/desktop/linux-bootstrap.sh').read_text(encoding='utf-8').split("<<'PY'\n", 1)[1].split('\nPY\n', 1)[0]
bootstrap = types.ModuleType('linux_bootstrap')
exec(compile(source, 'linux-bootstrap.sh:python', 'exec'), bootstrap.__dict__)


class Response(io.BytesIO):
    status = 200


@unittest.skipUnless(sys.platform == 'linux' and shutil.which('openssl'), 'Run in Linux with OpenSSL 3')
class Bootstrap(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.key = self.directory / 'private.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(self.key)], check=True, capture_output=True)
        public = subprocess.run(['openssl', 'pkey', '-in', str(self.key), '-pubout', '-outform', 'DER'], check=True, capture_output=True).stdout
        self.public = base64.b64encode(public[-32:]).decode()

    def manifest(self, data=b'package'):
        filename = 'pixels-agent-bridge-linux-x86_64-release-1.3.0.tar.gz'
        info = dict(schema=1, platform='linux', architecture='x86_64', channel='stable', version='1.3.0',
            filename=filename, size=len(data), sha256=hashlib.sha256(data).hexdigest(),
            download_url=f'https://fixture.cos.ap-beijing.myqcloud.com/releases/linux/x86_64/1.3.0/{filename}')
        canonical = f'PAB-RELEASE-V1\nlinux\nx86_64\nstable\n1.3.0\n{filename}\n{len(data)}\n{info["sha256"]}\n'
        message = self.directory / 'to-sign'
        message.write_bytes(canonical.encode())
        signature = subprocess.run(['openssl', 'pkeyutl', '-sign', '-rawin', '-inkey', str(self.key), '-in', str(message)], check=True, capture_output=True).stdout
        info['signature'] = base64.b64encode(signature).decode()
        return info

    def test_signature_and_tampered_metadata(self):
        info = self.manifest()
        bootstrap.validate_manifest(info, self.directory, self.public)
        for change in ({'sha256': '0' * 64}, {'architecture': 'aarch64'}, {'size': 0},
                       {'filename': '../package'}, {'download_url': 'http://example.test/package'},
                       {'download_url': info['download_url'] + '?token=secret'}, {'signature': 'bad'}):
            with self.subTest(change=change), self.assertRaises((RuntimeError, ValueError)):
                bootstrap.validate_manifest(dict(info, **change), self.directory, self.public)

    def test_incomplete_corrupt_and_oversized_downloads(self):
        info = self.manifest(b'correct')
        for content in (b'correct', b'corrupt', b'c', b'correct-too-long'):
            with self.subTest(content=content), patch.object(bootstrap, 'fetch', return_value=Response(content)):
                if content == b'correct':
                    bootstrap.download_package(info, self.directory / 'download')
                else:
                    with self.assertRaises(RuntimeError):
                        bootstrap.download_package(info, self.directory / 'download')

    def test_archive_rejects_links_traversal_duplicates_and_missing_files(self):
        for name, kind, duplicate in (('../escape', tarfile.REGTYPE, False), ('safe', tarfile.SYMTYPE, False),
                                      ('safe', tarfile.REGTYPE, True), ('other', tarfile.REGTYPE, False)):
            archive = self.directory / 'unsafe.tar.gz'
            with tarfile.open(archive, 'w:gz') as tar:
                member = tarfile.TarInfo(name)
                member.type = kind
                member.linkname = '/etc/passwd' if kind == tarfile.SYMTYPE else ''
                tar.addfile(member)
                if duplicate:
                    tar.addfile(member)
            destination = self.directory / 'extracted'
            with self.subTest(name=name, kind=kind, duplicate=duplicate), self.assertRaises(RuntimeError):
                bootstrap.extract_regular(archive, destination, {'safe'})
            self.assertFalse(destination.exists())

    def payload(self):
        elf = b'\x7fELF\x02\x01' + bytes(12) + b'\x3e\x00'
        runtime = io.BytesIO()
        with tarfile.open(fileobj=runtime, mode='w:gz') as tar:
            entry = tarfile.TarInfo('document-runtime/libpdfium.so')
            entry.size = len(elf)
            tar.addfile(entry, io.BytesIO(elf))
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode='w:gz') as tar:
            for name in bootstrap.REQUIRED:
                data = elf if name.startswith('pab-') else b'fixture'
                if name == 'document-runtime-linux.tar.gz':
                    data = runtime.getvalue()
                if name == 'release.json':
                    data = json.dumps(dict(platform='linux', architecture='x86_64', version='1.3.0')).encode()
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                tar.addfile(entry, io.BytesIO(data))
        return archive.getvalue()

    def test_install_only_after_verified_download_and_runtime_preflight(self):
        payload = self.payload()
        info = self.manifest(payload)
        original_verify = bootstrap.validate_manifest
        original_run = subprocess.run
        for corrupt, missing_library in ((False, False), (True, False), (False, True)):
            installed = []
            def run(command, **kwargs):
                if command[0] == 'openssl':
                    return original_run(command, **kwargs)
                if command[0] == 'ldd':
                    return subprocess.CompletedProcess(command, 0, 'libstdc++.so.6 => not found' if missing_library else 'libc.so.6 => /lib/libc.so.6', '')
                installed.append(command)
                return subprocess.CompletedProcess(command, 0)
            with self.subTest(corrupt=corrupt, missing_library=missing_library), \
                    patch.object(bootstrap, 'fetch', side_effect=[Response(json.dumps(info).encode()), Response(payload[:-1] if corrupt else payload)]), \
                    patch.object(bootstrap, 'validate_manifest', side_effect=lambda data, folder: original_verify(data, folder, self.public)), \
                    patch.object(bootstrap.platform, 'system', return_value='Linux'), \
                    patch.object(bootstrap.platform, 'machine', return_value='x86_64'), \
                    patch.object(bootstrap.os, 'geteuid', return_value=0, create=True), \
                    patch.object(bootstrap.shutil, 'which', return_value='/usr/bin/tool'), \
                    patch.object(bootstrap.subprocess, 'run', side_effect=run), \
                    patch.object(bootstrap.sys, 'argv', ['installer', '--no-service']):
                if corrupt or missing_library:
                    with self.assertRaises(RuntimeError):
                        bootstrap.main()
                    self.assertEqual(installed, [])
                else:
                    bootstrap.main()
                    self.assertEqual(len(installed), 1)
                    self.assertEqual(installed[0][0], 'bash')
                    self.assertEqual(installed[0][2:], ['--no-service'])


if __name__ == '__main__':
    unittest.main()
