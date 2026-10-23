"""Operator trust-boundary tests; all service actions use fake targets."""
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location('roll_relays', Path(__file__).parents[1] / 'roll_relays.py')
roll = importlib.util.module_from_spec(spec)
spec.loader.exec_module(roll)


class ArchiveTests(unittest.TestCase):
    def test_traversal_absolute_ads_links_duplicates_and_expansion_rejected(self):
        with tempfile.TemporaryDirectory() as work:
            work = Path(work)
            for bad in ['../evil', '/evil', 'C:/evil', 'safe\\evil', 'safe/name:ads', 'name.']:
                archive = work / 'bad.zip'
                with zipfile.ZipFile(archive, 'w') as out:
                    out.writestr('pab-relay-server.exe', 'binary')
                    entry=zipfile.ZipInfo('placeholder')
                    entry.filename=bad # Prevent Windows ZipInfo from normalizing the crafted backslash.
                    out.writestr(entry, 'payload')
                with self.subTest(bad=bad), self.assertRaises(ValueError):
                    roll.unpack_relay(archive, 'windows', work / 'out')
                self.assertFalse((work / 'out').exists(), 'Must validate whole archive before extracting any binary')
            archive = work / 'duplicate.zip'
            with zipfile.ZipFile(archive, 'w') as out:
                out.writestr('A', 'a');out.writestr('a', 'b')
            with self.assertRaises(ValueError):
                roll.unpack_relay(archive, 'windows', work / 'out')
            archive = work / 'link.tar.gz'
            with tarfile.open(archive, 'w:gz') as out:
                member = tarfile.TarInfo('link');member.type = tarfile.SYMTYPE;member.linkname = '/etc/passwd';out.addfile(member)
            with self.assertRaises(ValueError):
                roll.unpack_relay(archive, 'linux', work / 'out')

    def test_requires_both_binary_and_installer(self):
        with tempfile.TemporaryDirectory() as work:
            root = Path(work);archive = root / 'good.zip'
            with zipfile.ZipFile(archive, 'w') as out:
                out.writestr('pab-relay-server.exe', b'good')
                out.writestr('windows/install.ps1', 'installer')
            self.assertEqual(roll.unpack_relay(archive, 'windows', root / 'out'), roll.hashlib.sha256(b'good').hexdigest())

    def test_reserved_executor_and_non_dedicated_targets_rejected(self):
        for platform, root, service in [('windows', 'C:\\', 'relay'), ('linux', '/', 'relay'),
                ('windows', 'C:\\relay', 'PixelsAgentBridgeExecutor'), ('linux', '/opt/relay', 'pixels-agent-bridge-executor.service')]:
            with self.subTest(platform=platform, root=root), self.assertRaises(ValueError):
                roll.Target({'platform':platform, 'install_directory':root, 'service_name':service, 'config_file':'file'}, Path('.'))


class RecoveryTests(unittest.TestCase):
    def test_resume_unknown_update_outcome_reads_status_without_repeating_install(self):
        journal = {'nodes':{'a':{'phase':'deploying','stage':'stage','previous_sha256':'old','target_sha256':'new'}}}
        config = {'nodes':[{'node_id':'a','platform':'windows','artifact':'x'}]}
        actions = []; events = []
        class Target:
            def __init__(self, *args):pass
            def install(self, stage, action):
                actions.append(action)
                if action != 'status':raise AssertionError('Must not repeat an update with unknown outcome')
                return {'installer_active':False, 'running_sha256':'new'}
            def close(self):pass
        class Api:
            token = 'private'
            def nodes(self):return {'a':{'health':'healthy','admin_state':'active'}}
            def request(self, method, *args):
                events.append('ack' if method == 'GET' else 'activate')
                return {'phase':'completed'} if method=='GET' else {}
        with patch.object(roll, 'Target', Target), patch.object(roll, 'unpack_relay', return_value='new'), patch.object(roll, 'probe', side_effect=lambda *args: events.append('probe')), patch.object(roll.time, 'sleep'):
            roll.run_rollout(config, Path('.'), Api(), journal, lambda:None)
        self.assertEqual(journal['nodes']['a']['phase'], 'done')
        self.assertEqual(actions, ['status'])
        self.assertEqual(events, ['activate','ack','probe'])

    def test_superseded_activation_is_not_a_confirmation(self):
        class Api:
            def request(self, *args):return {'phase':'completed','error_code':'superseded'}
        with self.assertRaisesRegex(RuntimeError, 'not confirmed'):
            roll.wait_activation(Api(), 'owned', 10)

    def test_rollback_does_not_override_another_operators_maintenance(self):
        journal = {'nodes':{'a':{'phase':'restoring','operation_id':'owned'}}}
        class Api:
            def nodes(self):return {'a':{'admin_state':'maintenance','operation_id':'someone-else'}}
        with patch.object(roll, 'Target') as target:
            with self.assertRaisesRegex(RuntimeError, 'Another operator'):
                roll.recover({'nodes':[{'node_id':'a'}]}, Path('.'), Api(), journal, lambda:None)
            target.return_value.install.assert_not_called()


if __name__ == '__main__':unittest.main()
