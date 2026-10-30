"""Regression checks for Release smoke commands and native cleanup boundaries."""
from pathlib import Path
import unittest
from unittest.mock import patch
import uuid

from smoke.adapters import command_for
from smoke.cargo import environment, test_command
from smoke.live_native import temporary_parent, owned_temporary_path


class ReleaseSmokeTests(unittest.TestCase):
    def test_adapter_uses_fast_release_even_with_inherited_publication_environment(self):
        with patch('smoke.native_env.prepare', side_effect=lambda env: dict(env)), \
                patch('smoke.adapters.shutil.which', return_value='cargo'), \
                patch.dict('os.environ', {'PAB_BUILD_PRESET': 'optimized', 'CARGO_PROFILE_RELEASE_LTO': 'thin'}):
            cmd, _, env, parser = command_for('cargo-core', Path('.'), {})
        self.assertIn('--release', cmd)
        self.assertEqual(cmd[cmd.index('-j') + 1], '1')
        self.assertEqual(env['CARGO_PROFILE_RELEASE_LTO'], 'false')
        self.assertEqual(env['CARGO_PROFILE_RELEASE_OPT_LEVEL'], '2')
        self.assertEqual(env['CARGO_BUILD_JOBS'], '1')
        self.assertEqual(parser, 'libtest')

    def test_standalone_and_ci_share_release_command_and_bounded_environment(self):
        cmd = test_command(['-p', 'pab-executor', '--no-run'])
        self.assertIn('--release', cmd)
        self.assertIn('--no-run', cmd)
        env = environment({'CARGO_BUILD_JOBS': '10', 'CMAKE_BUILD_PARALLEL_LEVEL': '10'})
        self.assertEqual(env['CARGO_BUILD_JOBS'], '1')
        self.assertEqual(env['CMAKE_BUILD_PARALLEL_LEVEL'], '1')
        self.assertEqual(env['CARGO_INCREMENTAL'], '1')


class NativeTemporaryPaths(unittest.TestCase):
    def test_create_and_cleanup_use_same_real_parent_on_each_platform(self):
        for family, expected in [('windows', r'C:\Windows\Temp'), ('macos', '/private/tmp'), ('linux', '/tmp')]:
            parent = temporary_parent(family)
            self.assertEqual(str(parent), expected)
            path = parent / ('pab-smoke-' + str(uuid.uuid4()))
            self.assertEqual(owned_temporary_path(family, str(path)), path)

    def test_cleanup_rejects_alias_parent_non_uuid_and_other_directories(self):
        for path in ['/tmp/pab-smoke-' + str(uuid.uuid4()), '/private/tmp/ordinary',
                     '/private/tmp/pab-smoke-not-a-uuid', '/private/tmp/elsewhere/pab-smoke-' + str(uuid.uuid4())]:
            with self.assertRaises(ValueError):
                owned_temporary_path('macos', path)
