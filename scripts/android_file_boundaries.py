#!/usr/bin/env python3
"""Inject an owned symlink/readonly directory; verify denial through real remote tools."""
import argparse
import hashlib
import json
import time
import uuid
from pathlib import Path

from android_home import Home
from android_public import PublicMcp
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    run = uuid.uuid4().hex
    directory = ROOT / '.build/android/files' / ('boundaries-' + run)
    directory.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    remote = '/workspace/pixels-boundary-' + run
    native = 'no_backup' + remote
    private_name = 'pixels-boundary-private-' + run + '.bin'
    private = 'no_backup/' + private_name
    shell_stage = '/data/local/tmp/' + private_name
    fixture = directory / 'fixture.bin'
    fixture.write_bytes(b'owned boundary fixture; never credentials\n')
    expected = hashlib.sha256(fixture.read_bytes()).hexdigest()
    home = Home(args.serial)
    home.start()
    home.set_paused(False)
    client = PublicMcp(operator / 'mcp.toml', args.device_code, True, directory / 'mcp.log',
        args.mcp_executable or ROOT / '.build/android/files/tools-v2/pab-mcp.exe')
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'forced_relay': True, 'adb_fault_injection_only': True, 'checks': []}
    created = False

    def call(name, fields):
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        with (directory / 'operation-trace.jsonl').open('a', encoding='utf-8') as output:
            output.write(json.dumps({'tool': name, 'operation_ref': value.get('operation_ref'),
                'state': value.get('result', {}).get('state'), 'error': value.get('result', {}).get('error')}) + '\n')
        return value, raw

    def error(value):
        return value.get('result', {}).get('error', {}).get('code')

    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        value, raw = call('pab_mkdir', {'path': remote})
        assert not raw.get('isError') and value['result']['state'] == 'completed'
        created = True
        value, raw = call('pab_file_write', {'path': remote + '/original.txt', 'content': 'unchanged'})
        assert not raw.get('isError') and value['result']['state'] == 'completed'
        original_hash = value['result']['metadata']['sha256']
        adb(args.serial, 'push', str(fixture), shell_stage)
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', shell_stage, private)
        # This target is our own fixed fixture outside the exported workspace.
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'ln', '-s', '../../' + private_name, native + '/alias')
        for tool, values in (
            ('pab_file_stat', {'path': remote + '/alias'}),
            ('pab_file_read', {'path': remote + '/alias'}),
            ('pab_file_write', {'path': remote + '/alias', 'content': 'must not escape', 'overwrite': True}),
            ('pab_file_read', {'path': remote + '/alias/child'}),
        ):
            value, raw = call(tool, values)
            assert raw.get('isError') and error(value) == 'symlink_not_allowed'
        actual = adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum', private).split()[0]
        assert actual == expected
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', native + '/alias')
        result['checks'].append({'name': 'symlink_and_intermediate_link_cannot_escape_mount', 'passed': True})
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'chmod', '500', native)
        value, raw = call('pab_file_write', {'path': remote + '/original.txt', 'content': 'must not replace',
            'overwrite': True, 'expected_hash': original_hash})
        assert raw.get('isError') and error(value) == 'access_denied'
        value, raw = call('pab_file_read', {'path': remote + '/original.txt'})
        assert not raw.get('isError') and value['result']['metadata']['sha256'] == original_hash
        result['checks'].append({'name': 'readonly_directory_blocks_replacement_without_damaging_bytes', 'passed': True})
        result['status'] = 'passed'
    finally:
        # These are exact, generated, owned flat paths; never remove a user tree.
        if created:
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'chmod', '700', native, check=False)
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', native + '/alias', check=False)
            value, raw = call('pab_file_delete', {'path': remote, 'recursive': True})
            reference = value.get('operation_ref', {})
            deadline = time.monotonic() + 60
            while value.get('result', {}).get('state') in ('running', 'cancel_requested'):
                assert time.monotonic() < deadline, 'Query original cleanup; do not replay'
                value, raw = call('pab_get_operation', {'operation_id': reference['operation_id'], 'wait_ms': 1000})
            result['remote_fixture_cleanup'] = not raw.get('isError') and value.get('result', {}).get('state') == 'completed'
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', private, check=False)
        adb(args.serial, 'shell', 'rm', '-f', shell_stage, check=False)
        client.close()
        home.set_paused(True)
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
