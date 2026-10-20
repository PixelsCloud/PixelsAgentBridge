#!/usr/bin/env python3
"""Exercise download cancellation/resume and staging quota via public PAB.

ADB injects/removes one UUID-named sparse quota fixture in private staging. It
does not perform the tested upload, download, publication, hashing or deletion.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import time
import uuid
from android_home import Home, normal_status
from android_public import PublicMcp
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    run = ROOT / '.build/android/system' / ('download-' + str(uuid.uuid4()))
    run.mkdir(parents=True)
    source = run / 'source.bin'
    with source.open('wb') as output:
        for _ in range(8): output.write(os.urandom(1024 * 1024))
    with source.open('rb') as data: expected = hashlib.file_digest(data, 'sha256').hexdigest()
    remote = '/workspace/pixels-' + run.name + '.bin'
    destination = run / 'download.bin'
    quota_fixture = 'no_backup/file-staging/' + str(uuid.uuid4()) + '.part'
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'forced_relay': True, 'test_path': remote, 'sha256': expected, 'size': source.stat().st_size,
        'checks': [], 'test_file_cleanup_pending': False, 'quota_fixture_removed': False}
    home = Home(args.serial); home.start(); home.set_paused(False)
    client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'mcp.log', args.mcp_executable)

    def record(value):
        item = value.get('operation', {})
        return item.get('operation', item)

    def call(name, fields):
        value, raw = client.tool(name, {'device_code': args.device_code, **fields}, allow_error=True)
        item = record(value); reply = value.get('result', {})
        with (run / 'operation-trace.jsonl').open('a', encoding='utf-8') as output:
            output.write(json.dumps({'tool': name, 'request_id': fields.get('request_id'),
                'operation_ref': value.get('operation_ref'), 'state': item.get('state') or reply.get('state'),
                'offset': item.get('offset'), 'error': item.get('error') or reply.get('error') or value.get('error')}) + '\n')
        return value

    def wait(operation, terminal=('completed',), seconds=900):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = call('pab_get_operation', {'operation_id': operation, 'wait_ms': 1000})
            state = record(value).get('state') or value.get('result', {}).get('state')
            if state in terminal: return value
            assert state not in ('completed', 'failed', 'cancelled', 'interrupted'), (state, operation)
            time.sleep(.1)
        raise RuntimeError('Original operation did not reach the required known outcome: ' + operation)

    try:
        connected, raw = client.connect(); assert connected.get('connected') and not raw.get('isError')
        # A sparse file consumes the staging accounting budget without filling the phone.
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'truncate', '-s', str(2 * 1024**3), quota_fixture)
        assert int(adb(args.serial, 'shell', 'run-as', PACKAGE, 'stat', '-c', '%s', quota_fixture).strip()) == 2 * 1024**3
        failed_id = str(uuid.uuid4())
        call('pab_upload_file', {'source': str(source), 'destination': remote, 'request_id': failed_id, 'overwrite': False, 'wait_ms': 0})
        rejected = wait(failed_id, ('failed',), seconds=120)
        assert 'staging_quota_exceeded' in json.dumps(record(rejected))
        absent = call('pab_file_stat', {'path': remote})
        assert absent['result']['error']['code'] == 'not_found'
        result['checks'].append({'name': 'staging_quota_rejects_before_publication', 'passed': True, 'request_id': failed_id})
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', quota_fixture)
        result['quota_fixture_removed'] = True
        upload = str(uuid.uuid4())
        call('pab_upload_file', {'source': str(source), 'destination': remote, 'request_id': upload, 'overwrite': False, 'wait_ms': 0})
        result['test_file_cleanup_pending'] = True
        complete = record(wait(upload))
        assert complete['sha256'] == expected
        original = str(uuid.uuid4())
        call('pab_download_file', {'source': remote, 'destination': str(destination), 'request_id': original, 'overwrite': False, 'wait_ms': 0})
        deadline = time.monotonic() + 120
        while True:
            active = record(call('pab_get_operation', {'operation_id': original, 'wait_ms': 0}))
            assert active['state'] not in ('completed', 'failed', 'cancelled')
            if 65536 <= active['offset'] < result['size']: break
            assert time.monotonic() < deadline, 'No active bytes observed; cancellation is not accepted as tested'
            time.sleep(.05)
        observed = active['offset']
        call('pab_cancel_operation', {'operation_id': original})
        cancelled = record(wait(original, ('cancelled', 'interrupted'), seconds=120))
        assert not destination.exists(), 'A cancelled partial download must not publish the final path'
        result['checks'].append({'name': 'active_download_cancelled_without_publication', 'passed': True,
            'request_id': original, 'state': cancelled['state'], 'observed_prefix_bytes': observed})
        resumed = str(uuid.uuid4())
        call('pab_download_file', {'source': remote, 'destination': str(destination), 'request_id': resumed,
            'resume_from': original, 'overwrite': False, 'wait_ms': 0})
        completed = record(wait(resumed))
        with destination.open('rb') as data: actual = hashlib.file_digest(data, 'sha256').hexdigest()
        assert actual == expected == completed['sha256'] and destination.stat().st_size == result['size']
        result['checks'].append({'name': 'explicit_download_resume_and_full_hash', 'passed': True,
            'request_id': resumed, 'resume_from': original, 'sha256': actual})
        deletion = str(uuid.uuid4()); call('pab_file_delete', {'path': remote, 'request_id': deletion}); wait(deletion)
        absent = call('pab_file_stat', {'path': remote}); assert absent['result']['error']['code'] == 'not_found'
        result['test_file_cleanup_pending'] = False
        result['status'] = 'passed'
    finally:
        if not result['quota_fixture_removed']:
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', quota_fixture)
            result['quota_fixture_removed'] = True
        client.close(); home.set_paused(True)
        result['ending_status'] = normal_status(args.serial)
        (run / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'checks': len(result['checks']), 'result': str(run / 'result.json')}), flush=True)


if __name__ == '__main__': main()
