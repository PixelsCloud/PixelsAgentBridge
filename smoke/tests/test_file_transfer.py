import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from smoke.file_transfer import execute, remote_base, sha256, validate_lab


class FileTransferDriverTests(unittest.TestCase):
    def settings(self, root):
        binary = root / 'lab.exe'
        binary.write_bytes(b'synthetic native test binary')
        return {'isolated_network': True, 'mode': 'auto', 'relay_urls': ['https://relay.example.test'],
            'device': [{'role': role, 'code': code, 'os': 'windows', 'parent': 'D:\\PixelsSmoke',
                        'binary': str(binary), 'sha256': sha256(binary)}
                       for role, code in [('client', '111111111'), ('server', '222222222')]]}

    def test_lab_pins_artifacts_and_rejects_roots_and_implicit_networks(self):
        with TemporaryDirectory() as temporary:
            settings = self.settings(Path(temporary))
            validate_lab({'file_transfer': settings})
            device = dict(settings['device'][0], parent='D:\\')
            with self.assertRaises(ValueError):
                remote_base(device, '01234567-89ab-cdef-0123-456789abcdef')
            device['parent'] = 'D:\\test\\..\\outside'
            with self.assertRaises(ValueError):
                remote_base(device, '01234567-89ab-cdef-0123-456789abcdef')
            settings['isolated_network'] = False
            with self.assertRaises(ValueError):
                validate_lab({'file_transfer': settings})
            settings['isolated_network'] = True
            Path(settings['device'][0]['binary']).write_bytes(b'changed')
            with self.assertRaises(ValueError):
                validate_lab({'file_transfer': settings})

    def test_lost_launch_reply_is_not_replayed_and_cannot_delete_a_running_fixture(self):
        class Client:
            def __init__(self, digest):
                self.calls, self.digest = [], digest

            def call(self, name, arguments, **kwargs):
                self.calls.append((name, arguments))
                if name == 'pab_connect':
                    return {'connected': True}
                if name == 'pab_upload_file':
                    return {'operation': {'state': 'completed', 'sha256': self.digest}, 'complete': True}
                if name == 'pab_file_hash':
                    return {'result': {'state': 'completed', 'metadata': {'sha256': self.digest}}}
                if name == 'pab_file_delete':
                    if arguments.get('max_depth', 64) > 64 or arguments.get('max_entries', 4096) > 4096:
                        return {'error': {'code': 'invalid_arguments'}}
                if name == 'pab_run_command':
                    raise TimeoutError('Reply lost after possible acceptance')
                return {'result': {'state': 'completed'}}

            def observe(self, value, **kwargs):
                return value

        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            settings = self.settings(root)
            client = Client(settings['device'][0]['sha256'])
            with self.assertRaises(TimeoutError):
                execute(client, settings, root / 'evidence')
            launches = [args for name, args in client.calls if name == 'pab_run_command']
            self.assertEqual(len(launches), 1)
            self.assertIn('request_id', launches[0])
            self.assertFalse(any(name == 'pab_file_delete' and args['device_code'] == '111111111'
                                 for name, args in client.calls))
            report = json.loads((root / 'evidence/result.json').read_text(encoding='utf-8'))
            self.assertEqual(report['status'], 'fail')
            self.assertFalse(report['cleanup'][0]['complete'])
            self.assertTrue(report['cleanup'][1]['complete'])
            self.assertFalse(report['full_acceptance'])


if __name__ == '__main__':
    unittest.main()
