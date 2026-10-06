"""Headless artifact allowlist, provenance and shell permissions in an isolated repo."""
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase, main
import json
import shutil
import subprocess
import sys
import tarfile

from build_version import record_artifacts

ROOT = Path(__file__).resolve().parents[1]


class LinuxPackage(TestCase):
    def test_headless_archive_and_modified_binary_rejection(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            scripts = root / 'scripts'
            scripts.mkdir()
            shutil.copy2(ROOT / 'scripts/build_version.py', scripts)
            package = root / 'packaging/desktop'
            shutil.copytree(ROOT / 'packaging/desktop/unix', package / 'unix')
            shutil.copy2(ROOT / 'packaging/desktop/build.py', package)
            binaries = root / '.build/guest-desktop-linux-debug'
            binaries.mkdir(parents=True)
            files = {}
            for name in ('pab-mcp', 'pab-executor'):
                path = binaries / name
                path.write_bytes(b'fixture-' + name.encode())
                files[name] = path
            # Stale desktop output must not leak into the new headless archive.
            (binaries / 'pab-desktop').write_bytes(b'old GUI')
            record_artifacts(root, 'linux', 'debug', '1.2.26', files)
            command = [sys.executable, str(package / 'build.py'), '--platform', 'linux', '--profile', 'debug']
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            archive = root / '.build/packages/pixels-agent-bridge-linux-x86_64-debug.tar.gz'
            with tarfile.open(archive) as tar:
                self.assertEqual(set(tar.getnames()), {
                    'pab-mcp', 'pab-executor', 'install.sh', 'uninstall.sh',
                    'lifecycle.sh', 'run-mcp.sh', 'run-executor.sh', 'INSTALL-LINUX.txt',
                })
                for info in tar:
                    self.assertEqual(info.mode, 0o644 if info.name.endswith('.txt') else 0o755)
                    if info.name.endswith('.sh'):
                        self.assertNotIn(b'\r\n', tar.extractfile(info).read())
            manifest = json.loads((archive.parent / 'SHA256-debug.json').read_text())
            self.assertEqual(manifest[archive.name]['version'], '1.2.26')
            files['pab-mcp'].write_bytes(b'modified')
            result = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('changed since', result.stderr)


if __name__ == '__main__':
    main()
