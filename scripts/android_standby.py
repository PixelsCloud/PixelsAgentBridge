#!/usr/bin/env python3
"""Read-only public MCP standby sampling; resource review remains a separate gate.

Runs on an isolated operator without ADB. Expected locked/paused refusals are
recorded separately from successful observations and never counted as task success.
The phone receives no wake lock or input from this sampler.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import time
import uuid

from android_public import PublicMcp


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    parser.add_argument('--soak-id', required=True, type=uuid.UUID)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--seconds', type=int, default=28860)
    args = parser.parse_args()
    assert 60 <= args.seconds <= 86400
    root = args.root.resolve()
    assert root.name == str(args.run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert session['apk_sha256'] == args.apk_sha256 and session['normal_pairing'] is True
    directory = root / ('soak-' + str(args.soak_id))
    directory.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':args.apk_sha256,
        'public_server':True, 'forced_relay':True, 'operator_run_id':str(args.run_id)},
        'soak_id':str(args.soak_id), 'requested_seconds':args.seconds,
        'started_at_unix':time.time(), 'samples':0, 'locked_samples':0,
        'paused_samples':0, 'completed_observations':0, 'final_acceptance':False}
    client = None
    awake = None
    try:
        if os.name == 'nt':
            import ctypes
            awake = ctypes.windll.kernel32.SetThreadExecutionState
            awake.argtypes = [ctypes.c_uint]
            awake.restype = ctypes.c_uint
            assert awake(0x80000001), 'Could not keep the operator awake for the measurement'
        executable = root / ('pab-mcp.exe' if os.name == 'nt' else 'pab-mcp')
        client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
            directory / 'mcp.log', executable)
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        assert connected['target']['execution']['os_family'] == 'android'
        result['device_ref'] = connected['device_ref']
        start = time.monotonic()
        index = 0
        while True:
            sample_start = time.monotonic()
            # Capabilities is deliberately available while locked or paused.
            capabilities, _ = client.mobile(str(uuid.uuid4()), {'action':'capabilities'})
            assert capabilities['operation']['state'] == 'completed'
            value, _ = client.mobile(str(uuid.uuid4()), {'action':'observe'})
            operation = value['operation']
            assert operation['state'] == 'completed' or (
                operation['state'] == 'failed' and operation.get('error') in ('screen_locked', 'control_paused'))
            assert not operation['action_dispatched']
            assert capabilities.get('connection_path') == 'relay' and value.get('connection_path') == 'relay'
            event = {'index':index, 'elapsed_seconds':round(time.monotonic() - start, 3),
                'wall_time_unix':time.time(), 'request_id':operation['request_id'],
                'state':operation['state'], 'error':operation.get('error'),
                'action_dispatched':False, 'connection_path':'relay'}
            with (directory / 'standby-trace.jsonl').open('a', encoding='utf-8') as output:
                output.write(json.dumps(event) + '\n')
            result['samples'] += 1
            result['elapsed_seconds'] = event['elapsed_seconds']
            result['locked_samples'] += int(operation.get('error') == 'screen_locked')
            result['paused_samples'] += int(operation.get('error') == 'control_paused')
            result['completed_observations'] += int(operation['state'] == 'completed')
            (directory / 'progress.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
            print(f'Standby sample {index}: {event["elapsed_seconds"]}s {event["error"] or "observed"}', flush=True)
            if time.monotonic() - start >= args.seconds:
                break
            index += 1
            time.sleep(max(0, min(sample_start + 60 - time.monotonic(),
                args.seconds - (time.monotonic() - start))))
        result['status'] = 'passed'
    finally:
        if client:
            client.close()
        if awake:
            awake(0x80000000)
        result['finished_at_unix'] = time.time()
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'soak_id':str(args.soak_id), 'status':result['status'],
            'elapsed_seconds':result.get('elapsed_seconds'), 'samples':result['samples']}), flush=True)


if __name__ == '__main__':
    main()
