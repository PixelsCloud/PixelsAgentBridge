"""Ensure Linux Desktop publishing cannot substitute an unverified package."""
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

import release
import build_linux_desktop


class DesktopBuildWorkspaceTests(unittest.TestCase):
    def test_build_mounts_main_workspace_and_creates_only_versioned_outputs(self):
        with tempfile.TemporaryDirectory(prefix='pab-linux-workspace-') as temporary:
            root = Path(temporary)
            output = root / '.build/linux-desktop/1.0.2'
            commands = []

            def run(command, **kwargs):
                commands.append([str(part) for part in command])
                if command[:2] == ['docker', 'run']:
                    (output / 'pab-desktop').write_bytes(b'built GUI fixture')

            with patch.object(build_linux_desktop, 'ROOT', root), \
                 patch.object(build_linux_desktop, 'run', side_effect=run), \
                 patch.object(build_linux_desktop.shutil, 'which', return_value='npm'), \
                 patch.object(build_linux_desktop.subprocess, 'check_output', return_value=b'fixture-commit\n'), \
                 patch.object(sys, 'argv', ['build_linux_desktop.py', '--version', '1.0.2', '--skip-image-build']):
                build_linux_desktop.main()

            docker = next(command for command in commands if command[:2] == ['docker', 'run'])
            self.assertIn(f'type=bind,source={root},target=/src', docker)
            self.assertNotIn(f'type=bind,source={output / "source"},target=/src', docker)
            self.assertFalse((output / 'source').exists())
            self.assertNotIn('CMAKE_BUILD_PARALLEL_LEVEL=1', docker)
            self.assertIn('unset CMAKE_BUILD_PARALLEL_LEVEL', docker[-1])
            self.assertIn(f'CARGO_BUILD_JOBS={build_linux_desktop.JOBS}', docker)
            receipt = json.loads((output / 'build.json').read_text())
            self.assertEqual(receipt['source_commit'], 'fixture-commit')
            self.assertLessEqual(receipt['cargo_jobs'], 10)
            self.assertEqual(receipt['cmake_jobs'], receipt['cargo_jobs'])
            self.assertTrue(receipt['native_jobs_shared_with_cargo'])


class DesktopReleaseReceiptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='pab-desktop-release-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.directory = self.root / '.build/linux-desktop/1.0.1'
        self.directory.mkdir(parents=True)
        self.binary = self.directory / 'pab-desktop'
        header = bytearray(20)
        header[:6], header[18:20] = b'\x7fELF\x02\x01', b'\x3e\x00'
        self.binary.write_bytes(header)
        self.package = self.directory / 'pixels-agent-bridge-desktop-linux-x86_64-1.0.1.deb'
        self.package.write_bytes(b'package fixture')
        self.metadata = dict(version='1.0.1', platform='linux-desktop', architecture='x86_64', filename=self.package.name,
                             bytes=self.package.stat().st_size, sha256=release.digest(self.package), desktop_sha256=release.digest(self.binary))
        self.save()
        (self.directory / 'build.json').write_text(json.dumps(dict(version='1.0.1', platform='linux-desktop', architecture='x86_64', sha256=release.digest(self.binary))))
        packager = self.root / 'packaging/desktop/build_linux_desktop_package.py'
        packager.parent.mkdir(parents=True)
        shutil.copy2(release.ROOT / 'packaging/desktop/build_linux_desktop_package.py', packager)
        self.scope = patch.object(release, 'ROOT', self.root)
        self.scope.start()
        self.addCleanup(self.scope.stop)

    def save(self):
        (self.directory / 'package.json').write_text(json.dumps(self.metadata))

    def test_accepts_matching_package_and_gui_receipts(self):
        self.assertEqual(release.package_info('linux-desktop', 'x86_64', '1.0.1'),
                         (self.package, self.package.stat().st_size, release.digest(self.package)))

    def test_rejects_changed_deb(self):
        self.package.write_bytes(b'altered package')
        with self.assertRaises(RuntimeError):
            release.package_info('linux-desktop', 'x86_64', '1.0.1')

    def test_rejects_wrong_release_target(self):
        self.metadata['platform'] = 'linux'
        self.save()
        with self.assertRaises(RuntimeError):
            release.package_info('linux-desktop', 'x86_64', '1.0.1')

    def test_rejects_changed_gui(self):
        self.binary.write_bytes(self.binary.read_bytes() + b'changed')
        with self.assertRaises(ValueError):
            release.package_info('linux-desktop', 'x86_64', '1.0.1')

    def test_rejects_missing_gui_receipt(self):
        (self.directory / 'build.json').unlink()
        with self.assertRaises(FileNotFoundError):
            release.package_info('linux-desktop', 'x86_64', '1.0.1')


if __name__ == '__main__':
    unittest.main()
