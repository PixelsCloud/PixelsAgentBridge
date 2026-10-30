"""Exact candidate and normal-connection checks must fail closed without Debug fixtures."""
from contextlib import ExitStack
import copy
import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from smoke import android_release as target

CODE = '919383159'
PACKAGE = 'com.pixels.agent'


class Client:
    def __init__(self):
        self.records = {}
        self.commands = []
        self.bad_screen = False
        self.bad_query = False
        self.bad_disconnect = False

    def __enter__(self):
        return self

    def __exit__(self, *args):
        pass

    def call(self, name, args):
        self.commands.append((name, args))
        if name == 'pab_connect':
            return {'connected': True, 'connection_path': 'relay', 'target': {'execution': {'os_family': 'android'}}}
        if name == 'pab_disconnect':
            return {'disconnected': not self.bad_disconnect}
        if name != 'pab_mobile_control':
            raise AssertionError(name)
        request = args['request_id']
        if args['operation'] == 'get':
            value = copy.deepcopy(self.records[request])
            if self.bad_query:
                value['state'] = 'unconfirmed'
            return {'operation': value}
        action = args['command']['action']
        value = {'request_id': request, 'state': 'completed', 'action_dispatched': False}
        if action == 'capabilities':
            value['capabilities'] = {'actions': ['observe', 'query']}
        else:
            value['observation'] = {'package_name': PACKAGE, 'width': 1080, 'height': 2400,
                                    'elements': [{'name': '000000000' if self.bad_screen else '919 383 159'}]}
        self.records[request] = copy.deepcopy(value)
        return {'operation': value}


class ReleaseAndroidTests(unittest.TestCase):
    def test_usb_hash_and_debuggable_rejection_precede_ui_or_mcp(self):
        with patch.object(target, 'installed_apk_sha256', return_value='different'), patch.object(target, 'adb') as adb:
            with self.assertRaisesRegex(ValueError, 'differs'):
                target.verify_phone('serial', PACKAGE, CODE, 'candidate')
            adb.assert_not_called()
        with patch.object(target, 'installed_apk_sha256', return_value='candidate'), \
                patch.object(target, 'adb', return_value='flags=[ HAS_CODE DEBUGGABLE ]') as adb:
            with self.assertRaisesRegex(ValueError, 'non-debuggable'):
                target.verify_phone('serial', PACKAGE, CODE, 'candidate')
            self.assertEqual(adb.call_count, 1)

    def test_wrong_usb_device_still_removes_only_its_owned_snapshot(self):
        xml = '<hierarchy><node package="com.pixels.agent" text="000000000"/></hierarchy>'
        def adb(serial, *args):
            if args[:3] == ('shell', 'dumpsys', 'package'):
                return 'flags=[ HAS_CODE ]'
            return xml
        with patch.object(target, 'installed_apk_sha256', return_value='candidate'), \
                patch.object(target, 'adb', side_effect=adb) as call:
            with self.assertRaisesRegex(ValueError, 'device code'):
                target.verify_phone('serial', PACKAGE, CODE, 'candidate')
            args = call.call_args.args
            self.assertEqual(args[:4], ('serial', 'shell', 'rm', '-f'))
            self.assertTrue(args[4].startswith('/data/local/tmp/pab-release-'))
            self.assertFalse(any('run-as' in row.args for row in call.call_args_list))

    def campaign(self, client, *, preflight_error=None):
        with TemporaryDirectory() as temp, ExitStack() as stack:
            root = Path(temp)
            artifact = root / 'artifact'
            artifact.write_bytes(b'exact candidate')
            output = root / 'smoke/runs/test'
            stack.enter_context(patch.object(target, 'ROOT', root))
            verify = stack.enter_context(patch.object(target, 'verify_phone', side_effect=preflight_error))
            stack.enter_context(patch.object(target, 'installed_apk_sha256', return_value=hashlib.sha256(artifact.read_bytes()).hexdigest()))
            factory = stack.enter_context(patch.object(target, 'McpClient', return_value=client))
            error = None
            try:
                target.run('serial', artifact, artifact, artifact, PACKAGE, CODE, output)
            except (RuntimeError, ValueError) as caught:
                error = caught
            return error, json.loads((output / 'android-result.json').read_text()), json.loads((output / 'counts.json').read_text()), factory.call_count

    def test_normal_release_success_has_postconditions_and_cleanup(self):
        client = Client()
        error, result, counts, _ = self.campaign(client)
        self.assertIsNone(error)
        self.assertEqual(result['status'], 'pass')
        self.assertTrue(result['cleanup'])
        self.assertEqual(counts, {'passed': 7, 'failed': 0, 'skipped': 0})
        self.assertEqual({args['command']['action'] for name, args in client.commands if 'command' in args},
                         {'capabilities', 'observe', 'query'})

    def test_preflight_failure_never_starts_mcp(self):
        error, result, counts, started = self.campaign(Client(), preflight_error=ValueError('candidate mismatch'))
        self.assertIsNotNone(error)
        self.assertEqual(started, 0)
        self.assertEqual(result['status'], 'fail')
        self.assertEqual(counts['failed'], 1)

    def test_wrong_remote_original_result_or_cleanup_cannot_pass(self):
        for attribute in ('bad_screen', 'bad_query', 'bad_disconnect'):
            client = Client()
            setattr(client, attribute, True)
            error, result, counts, _ = self.campaign(client)
            self.assertIsNotNone(error, attribute)
            self.assertEqual(result['status'], 'fail')
            self.assertEqual(counts['failed'], 1)
            self.assertEqual(client.commands[-1][0], 'pab_disconnect')
