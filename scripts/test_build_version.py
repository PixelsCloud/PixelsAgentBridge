"""Version rollover, persistence, concurrency and package provenance regressions."""
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase, main
from unittest.mock import patch
import json
import plistlib
import subprocess
import sys
import tomllib

from build_version import (build_lock, next_version, parse_version, read_state,
                           record_artifacts, reserve_version, synchronize, verify_artifacts, write_json,
                           macos_artifacts)


def fixture(root):
    write_json(root / 'build-version.json', {'version': '1.2.0', 'build_count': 0})
    for relative, name in [('crates/core/Cargo.toml', 'pab-core'), ('apps/desktop/src-tauri/Cargo.toml', 'pab-desktop')]:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n\n[dependencies]\nserde = "1"\n')
    for relative in ['Cargo.lock', 'apps/desktop/src-tauri/Cargo.lock']:
        (root / relative).write_text('version = 4\n\n[[package]]\nname = "pab-core"\nversion = "0.1.0"\n\n[[package]]\nname = "serde"\nversion = "1.0.228"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n')
    for app in ['desktop', 'web']:
        folder = root / 'apps' / app
        folder.mkdir(parents=True, exist_ok=True)
        write_json(folder / 'package.json', {'name': f'pab-{app}', 'version': '0.1.0', 'scripts': {'build': 'kept'}})
        write_json(folder / 'package-lock.json', {'version': '0.1.0', 'packages': {'': {'version': '0.1.0'}, 'node_modules/example': {'version': '9.8.7'}}})
    write_json(root / 'apps/desktop/src-tauri/tauri.conf.json', {'version': '0.1.0', 'identifier': 'vip.rgaa.pab.desktop'})


class Versions(TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        fixture(self.root)

    def test_rollover(self):
        for before, after in [('1.2.0', '1.2.1'), ('1.2.98', '1.2.99'), ('1.2.99', '1.3.0'), ('1.99.99', '2.0.0')]:
            with self.subTest(before=before):
                self.assertEqual(next_version(before), after)

    def test_invalid_versions(self):
        for value in ['1.2', '1.100.0', '1.2.100', '01.2.3', '-1.2.3', '1.2.0-beta', None]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse_version(value)

    def test_first_build_and_persisted_increment(self):
        with build_lock(self.root):
            self.assertEqual(reserve_version(self.root), '1.2.0')
        with build_lock(self.root):
            self.assertEqual(reserve_version(self.root), '1.2.1')
        self.assertEqual(read_state(self.root)['build_count'], 2)

    def test_all_manifests_and_only_owned_lock_entries_change(self):
        synchronize(self.root, '2.0.0')
        self.assertEqual(tomllib.loads((self.root / 'crates/core/Cargo.toml').read_text())['package']['version'], '2.0.0')
        self.assertEqual(tomllib.loads((self.root / 'apps/desktop/src-tauri/Cargo.toml').read_text())['package']['version'], '2.0.0')
        for relative in ['Cargo.lock', 'apps/desktop/src-tauri/Cargo.lock']:
            packages = tomllib.loads((self.root / relative).read_text())['package']
            self.assertEqual([p['version'] for p in packages], ['2.0.0', '1.0.228'])
        for app in ['desktop', 'web']:
            folder = self.root / 'apps' / app
            package = json.loads((folder / 'package.json').read_text())
            self.assertEqual(package['version'], '2.0.0')
            self.assertEqual(package['scripts']['build'], 'kept')
            lock = json.loads((folder / 'package-lock.json').read_text())
            self.assertEqual(lock['version'], '2.0.0')
            self.assertEqual(lock['packages']['']['version'], '2.0.0')
            self.assertEqual(lock['packages']['node_modules/example']['version'], '9.8.7')
        self.assertEqual(json.loads((self.root / 'apps/desktop/src-tauri/tauri.conf.json').read_text())['version'], '2.0.0')

    def test_failed_build_number_is_not_reused(self):
        with patch('build_version.synchronize', side_effect=OSError('interrupted write')):
            with self.assertRaises(OSError):
                reserve_version(self.root)
        self.assertEqual(reserve_version(self.root), '1.2.1')
        self.assertEqual(tomllib.loads((self.root / 'crates/core/Cargo.toml').read_text())['package']['version'], '1.2.1')

    def test_parallel_build_rejected_and_lock_released(self):
        code = 'from pathlib import Path; from build_version import build_lock; import sys\nwith build_lock(Path(sys.argv[1])): pass'
        command = [sys.executable, '-c', code, str(self.root)]
        with build_lock(self.root):
            result = subprocess.run(command, cwd=Path(__file__).parent, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('Another product build', result.stderr)
        result = subprocess.run(command, cwd=Path(__file__).parent, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_package_uses_built_version_without_bumping(self):
        binary = self.root / 'program.exe'
        binary.write_bytes(b'compiled 1.2.0')
        files = {'program.exe': binary}
        record_artifacts(self.root, 'desktop', 'debug', '1.2.0', files)
        before = read_state(self.root)
        self.assertEqual(verify_artifacts(self.root, 'desktop', 'debug', files), '1.2.0')
        self.assertEqual(read_state(self.root), before)
        binary.write_bytes(b'different binary')
        with self.assertRaisesRegex(ValueError, 'changed since'):
            verify_artifacts(self.root, 'desktop', 'debug', files)

    def test_unrecorded_and_missing_artifacts_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Missing build record'):
            verify_artifacts(self.root, 'desktop', 'debug', {})
        record_artifacts(self.root, 'desktop', 'debug', '1.2.0', {})
        with self.assertRaisesRegex(ValueError, 'list changed'):
            verify_artifacts(self.root, 'desktop', 'debug', {'unknown': self.root / 'unknown'})

    def test_multi_component_build_allocates_once(self):
        import build
        commands = []
        with patch.object(build, 'ROOT', self.root), patch.object(build, 'run', side_effect=lambda cmd, cwd: commands.append(cmd)), patch.object(sys, 'argv', ['build.py', 'web', 'desktop-web', 'web']):
            build.main()
        self.assertEqual(read_state(self.root), {'version': '1.2.0', 'build_count': 1})
        self.assertEqual(len(commands), 2)
        self.assertTrue(all(cmd[-1] == 'build:assets' for cmd in commands))

    def test_linux_build_requires_no_node_and_records_only_headless_binaries(self):
        import build
        if build.os.name != 'nt':
            self.skipTest('Linux Docker orchestration currently runs on Windows')
        commands = []

        def fake_run(command, cwd=None):
            commands.append(command)
            folder = self.root / '.build/guest-desktop-linux-debug'
            folder.mkdir(parents=True, exist_ok=True)
            for name in ('pab-mcp', 'pab-executor'):
                (folder / name).write_bytes(b'fixture')

        with patch.object(build, 'ROOT', self.root), patch.object(build, 'run', side_effect=fake_run), patch.object(build.shutil, 'which', side_effect=lambda x: None if x in ('npm', 'node') else x), patch.object(sys, 'argv', ['build.py', 'linux']):
            build.main()
        self.assertEqual(len(commands), 1)
        self.assertIn('-ComponentsOnly', commands[0])
        record = json.loads((self.root / '.build/builds/linux-debug.json').read_text())
        self.assertEqual(set(record['files']), {'pab-mcp', 'pab-executor'})
        self.assertEqual(read_state(self.root)['build_count'], 1)

    def test_macos_both_architectures_share_one_reserved_version(self):
        import build
        cli = self.root / 'apps/desktop/node_modules/@tauri-apps/cli/tauri.js'
        cli.parent.mkdir(parents=True)
        cli.touch()
        commands = []

        def fake_run(command, cwd=None):
            commands.append([str(x) for x in command])
            if command[0] == 'cargo':
                target = command[command.index('--target') + 1]
                folder = self.root / 'target' / target / 'debug'
                folder.mkdir(parents=True)
                for name in ('pab-executor', 'pab-mcp'):
                    (folder / name).write_bytes(b'fixture')
            elif command[0] == 'node':
                target = command[command.index('--target') + 1]
                app = self.root / 'apps/desktop/src-tauri/target' / target / 'debug/bundle/macos/Pixels Agent Bridge.app'
                executable = app / 'Contents/MacOS/pab-desktop'
                executable.parent.mkdir(parents=True)
                executable.write_bytes(b'fixture')
                (app / 'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleShortVersionString': read_state(self.root)['version']}))

        with patch.object(build, 'ROOT', self.root), patch.object(build, 'run', side_effect=fake_run), patch.object(build.shutil, 'which', side_effect=lambda x: x), patch.object(build.subprocess, 'check_output', return_value='aarch64-apple-darwin\nx86_64-apple-darwin\n'), patch.object(sys, 'platform', 'darwin'), patch.object(sys, 'argv', ['build.py', 'macos', '--macos-arch', 'all', '--package']):
            with patch('macos_signing.load_identity', return_value='A' * 40), patch('macos_signing.sign') as signing:
                build.main()
                self.assertEqual(signing.call_count, 6)
        self.assertEqual(read_state(self.root), {'version': '1.2.0', 'build_count': 1})
        for architecture in ('aarch64', 'x86_64'):
            record = json.loads((self.root / f'.build/builds/macos-{architecture}-debug.json').read_text())
            self.assertEqual(record['version'], '1.2.0')
            self.assertEqual(len(record['files']), 4)
        self.assertEqual(sum(cmd[0] == 'cargo' for cmd in commands), 2)
        self.assertEqual(sum('packaging/desktop/build_macos_pkg.py' in cmd for cmd in commands), 2)
        self.assertTrue(all('--debug' in cmd for cmd in commands if cmd[0] == 'node'))

    def test_missing_macos_target_does_not_consume_version(self):
        import build
        cli = self.root / 'apps/desktop/node_modules/@tauri-apps/cli/tauri.js'
        cli.parent.mkdir(parents=True)
        cli.touch()
        with patch.object(build, 'ROOT', self.root), patch.object(build.shutil, 'which', side_effect=lambda x: x), patch.object(build.subprocess, 'check_output', return_value='aarch64-apple-darwin\n'), patch.object(sys, 'platform', 'darwin'), patch.object(sys, 'argv', ['build.py', 'macos', '--macos-arch', 'all']), self.assertRaises(SystemExit):
            with patch('macos_signing.load_identity', return_value='A' * 40):
                build.main()
        self.assertEqual(read_state(self.root)['build_count'], 0)

    def test_missing_signing_identity_does_not_consume_version(self):
        import build
        cli = self.root / 'apps/desktop/node_modules/@tauri-apps/cli/tauri.js'
        cli.parent.mkdir(parents=True)
        cli.touch()
        with patch.object(build, 'ROOT', self.root), patch.object(build.shutil, 'which', side_effect=lambda x: x), patch.object(sys, 'platform', 'darwin'), patch.object(sys, 'argv', ['build.py', 'macos', '--macos-arch', 'aarch64']), patch('macos_signing.load_identity', side_effect=RuntimeError('identity missing')):
            with self.assertRaisesRegex(RuntimeError, 'identity missing'):
                build.main()
        self.assertEqual(read_state(self.root)['build_count'], 0)

    def test_macos_architecture_selection(self):
        import build
        with patch.object(build.platform, 'machine', return_value='arm64'):
            self.assertEqual(build.macos_architectures('native'), ['aarch64'])
        self.assertEqual(build.macos_architectures('all'), ['aarch64', 'x86_64'])
        with self.assertRaises(ValueError):
            build.macos_architectures('unknown')

    def test_macos_record_detects_app_or_service_replacement(self):
        app = self.root / 'App'
        executable = app / 'Contents/MacOS/pab-desktop'
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b'app')
        (app / 'Contents/Info.plist').write_bytes(b'plist fixture')
        for name in ('pab-executor', 'pab-mcp'):
            (self.root / name).write_bytes(b'service')
        files = macos_artifacts(self.root, app)
        record_artifacts(self.root, 'macos-aarch64', 'debug', '1.2.0', files)
        for path in (executable, app / 'Contents/Info.plist', self.root / 'pab-mcp'):
            before = path.read_bytes()
            path.write_bytes(b'replaced')
            with self.assertRaisesRegex(ValueError, 'changed since'):
                verify_artifacts(self.root, 'macos-aarch64', 'debug', macos_artifacts(self.root, app))
            path.write_bytes(before)


if __name__ == '__main__':
    main()
