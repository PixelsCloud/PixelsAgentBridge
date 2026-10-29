#!/usr/bin/env python3
"""Run existing mobile primitives on the exact installed APK through public PAB."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import time
import uuid

from android_home import Home, normal_status
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, installed_apk_sha256


def capture_control(tasks, *, enable):
    direction = 'forward'
    for attempt in range(30):
        screen = tasks.query()
        assert screen.get('package_name') == PACKAGE, 'Capture controls must belong to our app'
        names = [node.get('name') for node in screen['elements']]
        stop = next((name for name in names if name in ('停止屏幕采集', 'Stop screen capture')), None)
        start = next((name for name in names if name in ('授权屏幕采集', 'Authorize screen capture')), None)
        view = next((name for name in names if name in ('屏幕查看','Screen viewing')), None)
        access = next((name for name in names if name in ('访问','Access')), None)
        if access and view is None and stop is None and start is None:
            tasks.act(access)
            time.sleep(.3)
            continue
        if view and stop is None and start is None:
            active = any(name in ('已就绪','Ready') for name in names)
            if active == enable:
                return False  # Preserve a pre-existing session or an already stopped state.
            tasks.act(view)
            if not enable:
                time.sleep(.3)
                continue  # The regular dialog exposes Stop screen capture.
            prompt = tasks.query()
            assert prompt.get('package_name') in ('com.android.systemui', 'com.android.permissioncontroller')
            assert any('Pixels' in (node.get('name') or '') for node in prompt['elements'])
            allow = next(node['name'] for node in prompt['elements'] if node.get('name') in
                ('立即开始','Start now','START NOW','允许','Allow'))
            tasks.act(allow)
            time.sleep(.3)
            return True
        if enable and stop: return False  # Preserve a session we did not create.
        if not enable and start: return False
        if not enable and stop:
            tasks.act(stop)
            return True
        if enable and start:
            tasks.act(start)
            prompt = tasks.query()
            assert prompt.get('package_name') in ('com.android.systemui', 'com.android.permissioncontroller')
            assert any('Pixels' in (node.get('name') or '') for node in prompt['elements'])
            allow = next(node['name'] for node in prompt['elements'] if node.get('name') in
                ('立即开始', 'Start now', 'START NOW', '允许', 'Allow'))
            tasks.act(allow)
            time.sleep(.3)
            return True
        containers = tasks.query(role='android.widget.ScrollView')['elements']
        assert len(containers) == 1
        outcome, _ = tasks.call({'action': 'scroll', 'element_ref': containers[0]['element_ref'],
            'direction': direction}, allow_pre_dispatch_errors=('target_stale', 'unsupported_action'))
        if outcome.get('error') == 'unsupported_action' or attempt == 14:
            direction = 'backward' if direction == 'forward' else 'forward'
    raise AssertionError('Normal capture control is unavailable')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--operator-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--device-code', required=True)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--mcp-executable', required=True, type=Path)
    parser.add_argument('--rounds', type=int, default=2, choices=range(1, 6))
    args = parser.parse_args()
    assert installed_apk_sha256(args.serial) == args.apk_sha256
    run = ROOT / '.build/android/execution' / ('mobile-' + str(uuid.uuid4()))
    run.mkdir(parents=True)
    operator = ROOT / '.build/android/public' / str(args.operator_run_id)
    result = {'status': 'failed', 'serial': args.serial, 'apk_sha256': args.apk_sha256,
        'mcp_sha256': hashlib.sha256(args.mcp_executable.read_bytes()).hexdigest(),
        'forced_relay': True, 'public_server': True, 'full_A01_A16': False,
        'qa_capture_started': False, 'qa_capture_stopped': False}
    home = Home(args.serial)
    previous_paused = normal_status(args.serial)['paused']
    client = None
    tasks = None
    try:
        home.start(); home.set_paused(False)
        client = PublicMcp(operator / 'mcp.toml', args.device_code, True, run / 'mcp.log', args.mcp_executable)
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, run / 'trace.jsonl')
        capability, _ = tasks.call({'action': 'capabilities'})
        if capability['capabilities']['screenshot_requires_consent']:
            result['qa_capture_started'] = capture_control(tasks, enable=True)
        result['basics'] = tasks.basics(args.rounds)
        assert installed_apk_sha256(args.serial) == args.apk_sha256
        result['status'] = 'passed'
    except Exception as error:
        result['error'] = str(error)[:1000]
        raise
    finally:
        try:
            home.start(); home.set_paused(False)
            if result['qa_capture_started']:
                assert tasks is not None
                capture_control(tasks, enable=False)
                result['qa_capture_stopped'] = True
            home.set_paused(previous_paused)
            if client is not None:
                client.tool('pab_disconnect', {'device_code': args.device_code})
            state = normal_status(args.serial)
            result['final_state'] = {key: state.get(key) for key in
                ('phase','remote_ready','accessibility_ready','paused','connection_requested')}
        except Exception as error:
            result['status'] = 'failed'
            result['cleanup_error'] = str(error)[:600]
        finally:
            if client is not None: client.close()
            (run / 'result.json').write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding='utf-8')
            print(json.dumps({'directory': str(run), 'status': result['status']}), flush=True)
        if result['status'] != 'passed': raise AssertionError('Mobile regression incomplete; see its result')


if __name__ == '__main__':
    main()
