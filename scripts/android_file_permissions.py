#!/usr/bin/env python3
"""Exercise normal read-only/revoke controls for the sole, owned acceptance grant."""
import argparse
import json
import re
import time
import uuid
from pathlib import Path

from android_home import Home
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('readonly', 'revoke'))
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--mount', required=True)
    parser.add_argument('--folder', required=True)
    parser.add_argument('--read-path', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', type=Path)
    args = parser.parse_args()
    assert re.fullmatch(r'PixelsAcceptance_[a-f0-9]{8}', args.folder)
    assert re.fullmatch(r'/shared-[a-f0-9]{16}', args.mount)
    assert args.read_path.startswith(args.mount + '/pixels-acceptance-')
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    code = json.loads((root / 'session.json').read_text(encoding='utf-8'))['device_code']
    directory = ROOT / '.build/android/files' / ('permissions-' + str(uuid.uuid4()))
    directory.mkdir(parents=True)
    home = Home(args.serial)
    home.start()
    home.set_paused(False)
    client = PublicMcp(root / 'mcp.toml', code, True, directory / 'mcp.log',
        args.mcp_executable or ROOT / '.build/android/files/tools-v2/pab-mcp.exe')
    result = {'status': 'failed', 'action': args.action, 'mount': args.mount,
        'apk_sha256': args.apk_sha256, 'normal_home_control': True, 'forced_relay': True}

    def call(name, values):
        return client.tool(name, {'device_code': code, **values}, allow_error=True)

    def listing():
        value, raw = call('pab_list_directory', {'path': '/', 'limit': 64})
        assert not raw.get('isError')
        return value.get('page', value)['entries']

    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        assert [item['name'] for item in listing() if item['name'].startswith('shared-')] == [args.mount[1:]], 'Do not operate on another grant'
        original, raw = call('pab_file_read', {'path': args.read_path})
        assert not raw.get('isError') and original['result']['state'] == 'completed'
        before_hash = original['result']['metadata']['sha256']
        tasks = AndroidTasks(client, PACKAGE, directory / 'operation-trace.jsonl')
        names = ('改为只读', 'Make read-only') if args.action == 'readonly' else ('撤销目录授权', 'Revoke directory access')
        direction = 'forward'
        for attempt in range(24):
            screen = tasks.query()
            assert screen['package_name'] == PACKAGE
            own = any(args.folder in (item.get('name') or '') and args.mount in (item.get('name') or '') for item in screen['elements'])
            controls = [item for item in screen['elements'] if item.get('name') in names and item['enabled']]
            if own and len(controls) == 1:
                tasks.act(controls[0]['name'])
                break
            scrolls = tasks.query(role='android.widget.ScrollView')['elements']
            assert len(scrolls) == 1, 'The normal home has no unique scroll container'
            outcome, _ = tasks.call({'action': 'scroll', 'element_ref': scrolls[0]['element_ref'],
                'direction': direction}, allow_pre_dispatch_errors=('target_stale', 'unsupported_action'))
            if outcome.get('error') == 'unsupported_action' or attempt == 11:
                direction = 'backward' if direction == 'forward' else 'forward'
            time.sleep(.3)
        else:
            raise RuntimeError('Own directory control could not be safely identified')
        time.sleep(.5)
        if args.action == 'readonly':
            value, raw = call('pab_file_read', {'path': args.read_path})
            assert not raw.get('isError') and value['result']['metadata']['sha256'] == before_hash
            assert value['result']['metadata']['readonly'] is True
            refused, raw = call('pab_file_write', {'path': args.read_path, 'content': 'must not replace',
                'overwrite': True, 'expected_hash': before_hash})
            assert raw.get('isError') and refused['result']['error']['code'] == 'access_denied'
            value, raw = call('pab_file_read', {'path': args.read_path})
            assert not raw.get('isError') and value['result']['metadata']['sha256'] == before_hash
            result['read_still_available_write_rejected_unchanged'] = True
        else:
            assert not any(item['name'] == args.mount[1:] for item in listing())
            value, raw = call('pab_file_read', {'path': args.read_path})
            assert raw.get('isError') and value['result']['error']['code'] == 'mount_not_found'
            result['revoked_mount_unavailable'] = True
        result['status'] = 'passed'
    finally:
        client.close()
        home.set_paused(True)
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
