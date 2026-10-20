#!/usr/bin/env python3
"""Grant only a new, explicitly named acceptance directory through the normal picker.

The normal home must already have opened ACTION_OPEN_DOCUMENT_TREE. This driver
uses real MCP gestures and clicks; it never writes URI grants or system settings.
"""
import argparse
import json
from pathlib import Path
import re
import time
import uuid

from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--folder', required=True)
    parser.add_argument('--mcp-executable', type=Path)
    parser.add_argument('--use-current-folder', action='store_true', help='Confirm the already selected owned folder; the final system dialog must still name it')
    args = parser.parse_args()
    assert re.fullmatch(r'PixelsAcceptance_[a-f0-9]{8}', args.folder)
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    directory = ROOT / '.build/android/files' / ('grant-' + str(uuid.uuid4()))
    directory.mkdir(parents=True)
    client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
        directory / 'mcp.log', args.mcp_executable or root / 'pab-mcp.exe')
    result = {'status': 'failed', 'normal_system_picker': True, 'folder': args.folder,
        'system_grants_written_directly': False, 'started_at_unix': time.time()}
    try:
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, directory / 'operation-trace.jsonl')

        def tap_label(name):
            screen = tasks.query()
            nodes = [node for node in screen['elements'] if node.get('name') == name]
            assert len(nodes) == 1, 'Directory label is ambiguous or absent'
            node = nodes[0]
            if node['clickable']:
                tasks.call({'action': 'click', 'element_ref': node['element_ref']})
            else:
                bounds = node['bounds']
                candidates = [item for item in screen['elements'] if item['clickable'] and item['enabled']
                    and item['bounds']['left'] <= bounds['left'] and item['bounds']['right'] >= bounds['right']
                    and item['bounds']['top'] <= bounds['top'] and item['bounds']['bottom'] >= bounds['bottom']]
                def area(item):
                    box = item['bounds']
                    return (box['right'] - box['left']) * (box['bottom'] - box['top'])
                assert candidates, 'Folder has no visible clickable container'
                smallest = min(map(area, candidates))
                candidates = [item for item in candidates if area(item) == smallest]
                assert len(candidates) == 1, 'Folder click target is ambiguous'
                tasks.call({'action': 'click', 'element_ref': candidates[0]['element_ref']})
            time.sleep(.4)

        def open_folder(name):
            direction = 'backward'
            for attempt in range(32):
                screen = tasks.query()
                assert screen.get('package_name') in ('com.google.android.documentsui', 'com.android.documentsui')
                if any(item.get('name') == name for item in screen['elements']):
                    tap_label(name)
                    return
                lists = [item for item in screen['elements'] if item.get('role') in
                    ('androidx.recyclerview.widget.RecyclerView', 'android.widget.GridView', 'android.support.v7.widget.RecyclerView')]
                if len(lists) > 1:
                    heights = [(item['bounds']['bottom'] - item['bounds']['top'], item) for item in lists]
                    tallest = max(height for height, _ in heights)
                    lists = [item for height, item in heights if height == tallest]
                if len(lists) != 1:
                    lists = tasks.query(role='android.widget.ScrollView')['elements']
                assert len(lists) == 1, 'Picker has no unique scroll container'
                outcome, _ = tasks.call({'action': 'scroll', 'element_ref': lists[0]['element_ref'],
                    'direction': direction},
                    allow_pre_dispatch_errors=('target_stale', 'unsupported_action'))
                if outcome.get('error') == 'unsupported_action' or attempt == 15:
                    direction = 'forward'
                time.sleep(.3)
            raise RuntimeError('Acceptance directory was not found in the system picker')

        if not args.use_current_folder:
            if not tasks.has(tasks.query(), args.folder):
                open_folder('Documents')
                print('Opened Documents', flush=True)
            open_folder(args.folder)
            print('Opened own acceptance folder', flush=True)
        screen = tasks.query()
        assert tasks.has(screen, args.folder), 'Acceptance folder is not visible'
        use = next((item['name'] for item in screen['elements']
            if item.get('name') in ('使用此文件夹', 'Use this folder', 'USE THIS FOLDER') and item['enabled']), None)
        assert use, 'The selected acceptance folder cannot be granted'
        tasks.act(use)
        screen = tasks.query()
        assert any(args.folder in (item.get('name') or '') and 'Pixels Android' in (item.get('name') or '')
            for item in screen['elements']), 'Unexpected consent target; refuse granting a parent directory'
        allow = next((item['name'] for item in screen['elements']
            if item.get('name') in ('允许', 'Allow', 'ALLOW') and item['enabled']), None)
        assert allow, 'The real system consent button is unavailable'
        tasks.act(allow)
        time.sleep(.5)
        value, raw = client.tool('pab_list_directory', {'device_code': session['device_code'], 'path': '/', 'limit': 64})
        assert not raw.get('isError')
        page = value.get('page', value)
        mounts = [item['name'] for item in page['entries'] if item['name'].startswith('shared-')]
        assert len(mounts) == 1, 'Do not guess among existing user grants'
        result.update(status='passed', mount='/' + mounts[0], forced_relay=True)
    finally:
        client.close()
        result['finished_at_unix'] = time.time()
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
