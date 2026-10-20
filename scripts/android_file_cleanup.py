#!/usr/bin/env python3
"""Remove only paths explicitly recorded by our file campaigns, through public MCP."""
import argparse
import json
import re
import time
import uuid

from android_home import Home
from android_public import PublicMcp
from test_android_remote import ROOT, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--runs', required=True, nargs='+', type=uuid.UUID)
    parser.add_argument('--apk-sha256', required=True)
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    roots = ROOT / '.build/android/files'
    paths = []
    for run in args.runs:
        record = json.loads((roots / str(run) / 'result.json').read_text(encoding='utf-8'))
        assert record['provenance']['serial'] == args.serial
        for path in record.get('test_paths', []):
            assert re.fullmatch(r'/(?:workspace|shared-[0-9a-f]{16})/pixels-acceptance-' + str(run)[:8] + r'(?:-中文\.txt|-utf16\.txt|-empty\.bin|\.bin)', path)
            paths.append((path, False))
        for path in record.get('test_directories', []):
            assert re.fullmatch(r'/(?:workspace|shared-[0-9a-f]{16})/pixels-acceptance-' + str(run)[:8] + '-tree', path)
            paths.append((path, True))
    operator = ROOT / '.build/android/public' / str(args.public_run_id)
    directory = roots / ('cleanup-' + str(uuid.uuid4()))
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'apk_sha256': args.apk_sha256, 'serial': args.serial,
        'forced_relay': True, 'paths': [], 'source_runs': [str(run) for run in args.runs]}
    home = Home(args.serial)
    home.start()
    home.set_paused(False)
    client = PublicMcp(operator / 'mcp.toml', args.device_code, True, directory / 'mcp.log', roots / 'tools-v2/pab-mcp.exe')

    def call(name, values):
        return client.tool(name, {'device_code': args.device_code, **values}, allow_error=True)

    def missing(value, raw):
        return raw.get('isError') and value.get('result', {}).get('error', {}).get('code') == 'not_found'

    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        for path, recursive in paths:
            value, raw = call('pab_file_stat', {'path': path})
            if missing(value, raw):
                result['paths'].append({'path': path, 'already_absent': True})
                continue
            assert not raw.get('isError') and value['result']['metadata']['kind'] == ('directory' if recursive else 'file')
            request = str(uuid.uuid4())
            value, raw = call('pab_file_delete', {'path': path, 'request_id': request, 'recursive': recursive})
            assert not raw.get('isError')
            deadline = time.monotonic() + 60
            while value['result']['state'] in ('running', 'cancel_requested'):
                assert time.monotonic() < deadline, 'Query original deletion; never replay'
                value, raw = call('pab_get_operation', {'operation_id': request, 'wait_ms': 1000})
            assert not raw.get('isError') and value['result']['state'] == 'completed'
            value, raw = call('pab_file_stat', {'path': path})
            assert missing(value, raw)
            result['paths'].append({'path': path, 'deleted': True, 'request_id': request})
        result['status'] = 'passed'
    finally:
        client.close()
        home.set_paused(True)
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'count': len(result['paths']), 'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
