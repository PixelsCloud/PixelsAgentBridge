#!/usr/bin/env python3
"""Cancel an active upload, explicitly resume its private prefix, and verify bytes."""
import argparse
import hashlib
import json
import os
import re
import time
import uuid

from android_home import Home
from android_public import PublicMcp
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--size-mib', type=int, default=128)
    args = parser.parse_args()
    assert 32 <= args.size_mib <= 256
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    run_id = str(uuid.uuid4())
    directory = ROOT / '.build/android/files' / ('recovery-' + run_id)
    directory.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    source = directory / 'source.bin'
    with source.open('wb') as output:
        for _ in range(args.size_mib):
            output.write(os.urandom(1024 * 1024))
    with source.open('rb') as data:
        expected = hashlib.file_digest(data, 'sha256').hexdigest()
    remote = '/workspace/pixels-transfer-' + run_id + '.bin'
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'forced_relay': True, 'test_path': remote, 'size': source.stat().st_size,
        'sha256': expected, 'checks': [], 'test_file_cleanup_pending': True}
    home = Home(args.serial)
    home.start()
    home.set_paused(False)
    client = PublicMcp(operator / 'mcp.toml', args.device_code, True, directory / 'mcp.log',
        ROOT / '.build/android/files/tools-v2/pab-mcp.exe')
    attempts = []

    def transfer_record(value):
        record = value.get('operation', {})
        return record.get('operation', record)

    def call(name, fields, allow_error=False):
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        record = transfer_record(value)
        reply = value.get('result', {})
        with (directory / 'operation-trace.jsonl').open('a', encoding='utf-8') as output:
            output.write(json.dumps({'tool': name, 'operation_ref': value.get('operation_ref'),
                'state': record.get('state') or reply.get('state'), 'offset': record.get('offset'),
                'size': record.get('size'), 'phase': value.get('operation', {}).get('phase'),
                'error': value.get('error') or reply.get('error')}) + '\n')
        if not allow_error:
            assert not raw.get('isError'), 'Inspect original operation; never replay an uncertain transfer'
        return value

    def get(operation):
        return call('pab_get_operation', {'operation_id': operation, 'wait_ms': 0}, allow_error=True)

    def state(value):
        return value.get('result', {}).get('state') or transfer_record(value).get('state')

    def wait(operation, terminal=('completed',), seconds=900):
        deadline = time.monotonic() + seconds
        while True:
            value = get(operation)
            current = state(value)
            if current in ('completed', 'failed', 'cancelled', 'interrupted'):
                assert current in terminal, f'Original operation ended {current}; inspect its record'
                return value
            assert time.monotonic() < deadline, 'Original operation unresolved; do not replay'
            time.sleep(.15)

    def staged():
        records = {}
        for name in adb(args.serial, 'shell', 'run-as', PACKAGE, 'ls', 'no_backup/file-staging').splitlines():
            assert re.fullmatch(r'[a-f0-9-]{36}\.part', name)
            inode, size = adb(args.serial, 'shell', 'run-as', PACKAGE, 'stat', '-c', '%i:%s', 'no_backup/file-staging/' + name).strip().split(':')
            records[name] = {'inode': int(inode), 'size': int(size)}
        return records

    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        before_staging = staged()
        original = str(uuid.uuid4())
        attempts.append(original)
        call('pab_upload_file', {'source': str(source), 'destination': remote,
            'request_id': original, 'overwrite': False, 'wait_ms': 0})
        deadline = time.monotonic() + 90
        while True:
            value = get(original)
            operation = transfer_record(value)
            offset = operation['offset']
            assert operation['state'] not in ('completed', 'failed', 'cancelled')
            if 65536 <= offset < result['size']:
                break
            assert time.monotonic() < deadline, 'No active upload bytes observed; cancellation is not accepted as tested'
            time.sleep(.05)
        result['observed_prefix_bytes_before_cancel'] = offset
        call('pab_cancel_operation', {'operation_id': original}, allow_error=True)
        cancelled = wait(original, ('cancelled', 'interrupted', 'failed'), seconds=90)
        result['checks'].append({'name': 'active_upload_cancelled', 'passed': True, 'request_id': original,
            'state': state(cancelled), 'observed_prefix_bytes': offset})
        absent = call('pab_file_stat', {'path': remote}, allow_error=True)
        assert absent['result']['state'] == 'failed' and absent['result']['error']['code'] == 'not_found'
        prefixes = {name: value for name, value in staged().items() if name not in before_staging}
        assert len(prefixes) == 1, 'Retained prefix cannot be uniquely identified'
        prefix_name, prefix = next(iter(prefixes.items()))
        assert prefix['size'] >= offset and prefix['size'] < result['size']
        resumed = str(uuid.uuid4())
        attempts.append(resumed)
        call('pab_upload_file', {'source': str(source), 'destination': remote,
            'request_id': resumed, 'overwrite': False, 'resume_from': original, 'wait_ms': 0})
        time.sleep(.3)
        continuing = staged().get(prefix_name)
        assert continuing and continuing['inode'] == prefix['inode'] and continuing['size'] >= prefix['size'], 'Resumption did not retain the owned prefix'
        result['prefix_reuse_metadata'] = {'before': prefix, 'after_resuming': continuing,
            'adb_metadata_only': True, 'file_actions_through_pab': True}
        completed = wait(resumed)
        result['checks'].append({'name': 'explicit_upload_resume_completed', 'passed': True,
            'request_id': resumed, 'resume_from': original, 'sha256': completed['operation']['sha256']})
        assert completed['operation']['sha256'] == expected
        downloaded = directory / 'downloaded.bin'
        download = str(uuid.uuid4())
        attempts.append(download)
        call('pab_download_file', {'source': remote, 'destination': str(downloaded),
            'request_id': download, 'overwrite': False, 'wait_ms': 0})
        wait(download)
        with downloaded.open('rb') as data:
            assert hashlib.file_digest(data, 'sha256').hexdigest() == expected
        assert downloaded.stat().st_size == result['size']
        result['checks'].append({'name': 'resumed_file_download_matches_full_source', 'passed': True})
        hash_request = str(uuid.uuid4())
        attempts.append(hash_request)
        call('pab_file_hash', {'path': remote, 'request_id': hash_request})
        call('pab_cancel_operation', {'operation_id': hash_request}, allow_error=True)
        hashed = wait(hash_request, ('cancelled', 'completed'), seconds=120)
        if state(hashed) == 'completed':
            assert hashed['result']['metadata']['sha256'] == expected
        result['checks'].append({'name': 'hash_cancellation_terminal_state', 'passed': True,
            'request_id': hash_request, 'state': state(hashed), 'late_completion_preserved': state(hashed) == 'completed'})
        # All mutations have a known terminal state before cleaning our own file.
        deletion = str(uuid.uuid4())
        call('pab_file_delete', {'path': remote, 'request_id': deletion})
        wait(deletion, seconds=120)
        absent = call('pab_file_stat', {'path': remote}, allow_error=True)
        assert absent['result']['error']['code'] == 'not_found'
        result['test_file_cleanup_pending'] = False
        result['status'] = 'passed'
    finally:
        result['request_ids'] = attempts
        client.close()
        home.set_paused(True)
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'checks': len(result['checks']),
            'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
