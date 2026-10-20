#!/usr/bin/env python3
"""Regress stale Compose viewport bounds through actual MCP scroll/query pairs."""
import argparse
import json
import time
import uuid

from android_home import Home
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--rounds', type=int, default=20)
    args = parser.parse_args()
    assert 1 <= args.rounds <= 100
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert installed_apk_sha256(args.serial) == session['apk_sha256']
    adb(args.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n',
        f'{PACKAGE}/com.pixels.agent.MainActivity')
    home = Home(args.serial)
    home.start()
    if home.last_snapshot.get('paused'):
        home.click('resume')
    campaign = root / ('compose-scroll-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True}, 'rounds':[]}
    client = PublicMcp(root / 'mcp.toml', session['device_code'], True, campaign / 'mcp.log', root / 'pab-mcp.exe')
    try:
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
        time.sleep(.5)
        top = '打开验收页面'
        disconnect = '断开连接'

        def scroll(direction):
            # Every attempt uses a fresh explicit reference. Only a definite stale
            # rejection before dispatch permits another attempt.
            for _ in range(3):
                screen = tasks.query(role='android.widget.ScrollView')
                assert len(screen['elements']) == 1
                reply, _ = tasks.call({'action':'scroll',
                    'element_ref':screen['elements'][0]['element_ref'], 'direction':direction},
                    allow_pre_dispatch_errors=('target_stale',))
                if reply.get('error') == 'target_stale':
                    time.sleep(.2)
                    continue
                assert reply['platform_acknowledged'] is True
                return
            raise RuntimeError('Compose scroll target kept changing before dispatch')

        tasks.wait(top)
        for index in range(args.rounds):
            assert tasks.has(tasks.query(text=top), top)
            scroll('forward')
            time.sleep(.3)
            assert not tasks.has(tasks.query(text=top), top), 'Compose retained pre-scroll visible nodes'
            assert tasks.has(tasks.query(text=disconnect), disconnect)
            scroll('backward')
            tasks.wait(top)
            result['rounds'].append({'index':index, 'forward_viewport_updated':True,
                'backward_viewport_updated':True})
            print(f'Compose viewport {index + 1}/{args.rounds} passed', flush=True)
        result['status'] = 'passed'
    finally:
        client.close()
        (campaign / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'],
            'passed_rounds':len(result['rounds'])}))


if __name__ == '__main__':
    main()
