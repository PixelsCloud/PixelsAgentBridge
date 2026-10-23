"""Independent product orchestration, archive contents and provenance checks."""
from contextlib import ExitStack
import json
from pathlib import Path
import re
import sys
import tarfile
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import build
from build_version import record_artifacts
from package_services import deployment_files, package
from service_products import artifacts


class BuildProducts(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'build-version.json').write_text('{"version":"1.2.0","build_count":0}')
        self.commands = []

    def exercise(self, targets, *, unavailable=()):
        def fake_run(command, cwd=None):
            self.commands.append([str(x) for x in command])
            if command[0] == 'cargo':
                folder = self.root / 'target/debug'
                folder.mkdir(parents=True, exist_ok=True)
                suffix = '.exe' if build.os.name == 'nt' else ''
                for i, value in enumerate(command[:-1]):
                    if value == '--bin':
                        (folder / (command[i + 1] + suffix)).write_bytes(b'fixture binary')
            elif 'build:assets' in command:
                web = self.root / 'apps/web/dist'
                web.mkdir(parents=True)
                (web / 'index.html').write_text('<html>fixture</html>')

        with ExitStack() as stack:
            stack.enter_context(patch.object(build, 'ROOT', self.root))
            stack.enter_context(patch.object(build, 'CARGO_TARGET', self.root / 'target'))
            stack.enter_context(patch.object(build, 'run', side_effect=fake_run))
            stack.enter_context(patch.object(build.shutil, 'which', side_effect=lambda name: None if name in unavailable else name))
            stack.enter_context(patch.object(sys, 'argv', ['build.py', *targets]))
            build.main()

    def receipt(self, target):
        return json.loads((self.root / f'.build/builds/{target}-debug.json').read_text())

    def test_relay_build_and_package_without_node_or_web(self):
        self.exercise(['relay', '--package'], unavailable=('npm', 'node'))
        self.assertEqual(len(self.commands), 2)
        cargo, packager = self.commands
        self.assertIn('pab-relay', cargo)
        self.assertNotIn('pab-server', cargo)
        self.assertNotIn('pab-executor', cargo)
        self.assertLessEqual(int(cargo[cargo.index('-j') + 1]), 10)
        self.assertEqual(packager[packager.index('--product') + 1], 'relay')
        self.assertEqual({Path(name).stem for name in self.receipt('relay')['files']},
                         {'pab-relay-server', 'pab-relay-healthcheck'})
        self.assertFalse((self.root / 'apps/web').exists())

    def test_management_build_excludes_relay_and_client(self):
        self.exercise(['management', '--package'])
        cargo = next(command for command in self.commands if command[0] == 'cargo')
        self.assertIn('pab-server', cargo)
        self.assertNotIn('pab-relay', cargo)
        self.assertNotIn('pab-executor', cargo)
        self.assertEqual(len(self.receipt('management')['files']), 2)
        self.assertIn('web/index.html', self.receipt('management')['files'])

    def test_management_requires_web_toolchain_before_reserving_version(self):
        with self.assertRaises(SystemExit):
            self.exercise(['management'], unavailable=('npm',))
        self.assertEqual(json.loads((self.root / 'build-version.json').read_text())['build_count'], 0)

    def test_combined_build_preserves_legacy_target(self):
        self.exercise(['server'])
        self.assertEqual(len(self.receipt('server')['files']), 4)

    def test_two_products_have_distinct_receipts_without_consuming_client_versions(self):
        self.exercise(['management', 'relay'])
        self.assertEqual(self.receipt('management')['version'], self.receipt('relay')['version'])
        self.assertEqual(json.loads((self.root / 'build-version.json').read_text())['build_count'], 0)

    def test_mixed_build_allows_products_at_different_versions(self):
        self.exercise(['relay'])
        self.exercise(['relay'])
        self.exercise(['management', 'relay'])
        self.assertEqual(self.receipt('management')['version'], '1.2.0')
        self.assertEqual(self.receipt('relay')['version'], '1.2.2')

    def test_web_and_management_share_one_product_reservation_and_frontend_build(self):
        self.exercise(['web', 'management'])
        self.assertEqual(sum('build:assets' in command for command in self.commands), 1)
        self.assertEqual(self.receipt('management')['version'], '1.2.0')

    def test_docker_targets_need_only_docker_and_use_distinct_stages_and_tags(self):
        self.exercise(['docker-management', 'docker-relay', '--profile', 'release'], unavailable=('npm', 'node', 'cargo'))
        self.assertEqual(len(self.commands), 2)
        for product, command in zip(('management', 'relay'), self.commands):
            self.assertEqual(command[command.index('--target') + 1], product)
            self.assertEqual(command[command.index('-t') + 1], f'pixels-agent-bridge-{product}:1.2.0')

    def test_single_image_override_cannot_overwrite_two_product_images(self):
        with self.assertRaises(SystemExit):
            self.exercise(['docker-management', 'docker-relay', '--profile', 'release', '--image', 'fixture:same'])
        self.assertFalse(self.commands)


class ProductArchives(unittest.TestCase):
    def setUp(self):
        self.temp = TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.binary_dir = self.root / 'target/release'
        self.binary_dir.mkdir(parents=True)
        for suffix in ('', '.exe'):
            for name in ('pab-server', 'pab-relay-server', 'pab-relay-healthcheck', 'pab-executor'):
                (self.binary_dir / (name + suffix)).write_bytes((name + suffix).encode())

    def prepare(self, product, host):
        if product in ('management', 'server'):
            web = self.root / 'apps/web/dist'
            web.mkdir(parents=True, exist_ok=True)
            (web / 'index.html').write_text('<html>fixture</html>')
            (web / 'app.js').write_text('console.log("fixture")')
        for path in deployment_files(self.root, product, host).values():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('fixture deployment input')
        files = artifacts(self.root, product, host, self.binary_dir)
        record_artifacts(self.root, product, 'release', '1.2.3', files)

    def make(self, product, host):
        return package(self.root, product, host, 'release', self.binary_dir, self.root / 'out', 'x86_64')

    def test_relay_packages_contain_only_relay_binaries_and_no_web_or_server_config(self):
        for host in ('windows', 'linux'):
            with self.subTest(host=host):
                self.prepare('relay', host)
                result = self.make('relay', host)
                archive = Path(result['archive'])
                if host == 'windows':
                    with zipfile.ZipFile(archive) as source:
                        names = set(source.namelist())
                else:
                    with tarfile.open(archive) as source:
                        names = set(source.getnames())
                        self.assertEqual(source.getmember('pab-relay-server').mode, 0o755)
                        self.assertEqual(source.getmember('pab-relay-healthcheck').mode, 0o755)
                        self.assertEqual(source.getmember('LICENSE').mode, 0o644)
                self.assertFalse(any(name.startswith('web/') or 'pab-executor' in name for name in names))
                self.assertFalse(any(Path(name).name in ('pab-server', 'pab-server.exe', 'server.example.toml') for name in names))
                self.assertIn('roll_relays.py', names)
                manifest = json.loads(archive.with_suffix(archive.suffix + '.manifest.json').read_text())
                self.assertEqual(set(manifest['files']), names)
                self.assertEqual(manifest['product'], 'relay')
                self.assertIn('1.2.3', archive.name)
        self.assertFalse((self.root / 'apps/web').exists())

    def test_management_archive_contains_web_without_relay_or_client(self):
        self.prepare('management', 'windows')
        with zipfile.ZipFile(self.make('management', 'windows')['archive']) as source:
            names = set(source.namelist())
        self.assertIn('pab-server.exe', names)
        self.assertIn('web/index.html', names)
        self.assertIn('windows/server.example.toml', names)
        self.assertIn('LICENSE', names)
        self.assertFalse(any('pab-relay' in name or 'pab-executor' in name or 'relay.example' in name for name in names))

    def test_legacy_combined_archive_keeps_its_name_and_all_components(self):
        self.prepare('server', 'windows')
        result = self.make('server', 'windows')
        self.assertEqual(Path(result['archive']).name, 'pixels-agent-bridge-server-windows-x86_64-release.zip')
        with zipfile.ZipFile(result['archive']) as source:
            self.assertTrue({'pab-server.exe', 'pab-relay-server.exe', 'pab-relay-healthcheck.exe', 'web/index.html'} <= set(source.namelist()))

    def test_changed_binary_or_web_assets_cannot_reuse_receipt(self):
        self.prepare('management', 'windows')
        binary = self.binary_dir / 'pab-server.exe'
        original = binary.read_bytes()
        binary.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'changed since'):
            self.make('management', 'windows')
        binary.write_bytes(original)
        (self.root / 'apps/web/dist/new.js').write_text('new asset')
        with self.assertRaisesRegex(ValueError, 'list changed'):
            self.make('management', 'windows')

    def test_combined_receipt_cannot_authorize_relay_archive(self):
        self.prepare('server', 'windows')
        with self.assertRaisesRegex(ValueError, 'Missing build record'):
            self.make('relay', 'windows')

    def test_unexpected_web_files_are_rejected(self):
        self.prepare('management', 'windows')
        (self.root / 'apps/web/dist/private.toml').write_text('not a public asset')
        with self.assertRaisesRegex(ValueError, 'Unexpected production Web asset'):
            self.make('management', 'windows')

    def test_font_license_ships_but_other_text_files_are_rejected(self):
        self.prepare('management', 'windows')
        web = self.root / 'apps/web/dist'
        license = web / 'assets/OFL-10Pixel.txt'
        license.parent.mkdir()
        license.write_text('SIL OPEN FONT LICENSE Version 1.1')
        files = artifacts(self.root, 'management', 'windows', self.binary_dir)
        record_artifacts(self.root, 'management', 'release', '1.2.3', files)
        with zipfile.ZipFile(self.make('management', 'windows')['archive']) as source:
            self.assertEqual(source.read('web/assets/OFL-10Pixel.txt'), license.read_bytes())
        (web / 'assets/debug-notes.txt').write_text('not a public asset')
        with self.assertRaisesRegex(ValueError, 'Unexpected production Web asset'):
            artifacts(self.root, 'management', 'windows', self.binary_dir)

    def test_symlinked_binary_is_rejected(self):
        self.prepare('relay', 'windows')
        real = Path.is_symlink
        with patch.object(Path, 'is_symlink', lambda path: path.name == 'pab-relay-server.exe' or real(path)):
            with self.assertRaisesRegex(ValueError, 'non-regular build artifact'):
                self.make('relay', 'windows')


class DockerProducts(unittest.TestCase):
    def test_product_stage_dependencies_exclude_other_product_and_relay_never_builds_web(self):
        source = (ROOT / 'packaging/docker/Dockerfile').read_text()
        stages = {}
        current = None
        for line in source.splitlines():
            match = re.match(r'FROM (\S+) AS (\S+)', line)
            if match:
                base, current = match.groups()
                stages[current] = {'base': base, 'lines': []}
            elif current:
                stages[current]['lines'].append(line)

        def dependencies(name):
            stage = stages[name]
            direct = set(re.findall(r'--from=(\S+)', '\n'.join(stage['lines'])))
            if stage['base'] in stages:
                direct.add(stage['base'])
            return direct | set().union(*(dependencies(item) for item in direct))

        relay = dependencies('relay')
        management = dependencies('management')
        self.assertIn('relay-builder', relay)
        self.assertFalse({'web-builder', 'management-builder'} & relay)
        self.assertIn('web-builder', management)
        self.assertNotIn('relay-builder', management)
        self.assertTrue({'management-builder', 'relay-builder', 'web-builder'} <= dependencies('runtime'))
        for stage in ('relay-builder', 'management-builder'):
            self.assertIn('target=/src/target,sharing=locked', '\n'.join(stages[stage]['lines']))


if __name__ == '__main__':
    unittest.main()
