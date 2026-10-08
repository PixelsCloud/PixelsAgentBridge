"""Real Cargo cache regression for independent installer release versions."""
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import TestCase, main
import json
import shutil
import subprocess

from build_version import reserve_version, write_json


class InstallerVersionCache(TestCase):
    def test_release_bump_reuses_all_internal_components(self):
        if not shutil.which('cargo'):
            self.skipTest('Cargo is not installed')
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            write_json(root / 'build-version.json', {'version': '1.2.48', 'build_count': 49})
            (root / 'Cargo.toml').write_text('[workspace]\nresolver="2"\nmembers=["core","executor","bridge","desktop"]\n')
            for name in ('core', 'executor', 'bridge', 'desktop'):
                folder = root / name
                (folder / 'src').mkdir(parents=True)
                dependency = '' if name == 'core' else 'pab-core = { path = "../core" }\n'
                (folder / 'Cargo.toml').write_text(f'[package]\nname="pab-{name}"\nversion="0.1.0"\nedition="2024"\n[dependencies]\n{dependency}')
                (folder / 'src/lib.rs').write_text('pub fn sample() -> u8 { 1 }\n')
            command = ['cargo', 'build', '--offline', '--message-format=json', '--workspace']
            first = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=90)
            self.assertEqual(first.returncode, 0, first.stderr)
            manifests = [root / 'Cargo.lock', root / 'Cargo.toml', *root.glob('*/Cargo.toml')]
            before = {path: path.read_bytes() for path in manifests}
            self.assertEqual(reserve_version(root), '1.2.49')
            self.assertEqual({path: path.read_bytes() for path in manifests}, before)
            second = subprocess.run(command + ['--locked'], cwd=root, capture_output=True, text=True, timeout=90)
            self.assertEqual(second.returncode, 0, second.stderr)
            artifacts = [item for line in second.stdout.splitlines() if (item := json.loads(line)).get('reason') == 'compiler-artifact']
            self.assertEqual(len(artifacts), 4)
            self.assertTrue(all(item['fresh'] for item in artifacts), second.stdout)


if __name__ == '__main__':
    main()
