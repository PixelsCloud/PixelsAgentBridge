#!/usr/bin/env python3
"""Check ordinary phone pause/home/notification controls on the real public session."""
from __future__ import annotations

import argparse
import json
import time
import tomllib
import uuid

from android_home import Home
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from android_upgrade import phone_state
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    args = parser.parse_args()
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert installed_apk_sha256(args.serial) == session['apk_sha256']
    assert not adb(args.serial, 'reverse', '--list').strip()
    campaign = root / ('public-controls-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True, 'forced_relay':True}, 'checks':{}}
    home = Home(args.serial)
    client = None

    def show_home():
        adb(args.serial, 'shell', 'am', 'start', '--activity-reorder-to-front', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        time.sleep(.5)

    def intent():
        return tomllib.loads(adb(args.serial, 'shell', 'run-as', PACKAGE,
            'cat', 'no_backup/control-intent.toml'))

    def stopped():
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            home.tree()
            if home.last_snapshot.get('phase') == 'DISCONNECTED' and not home.last_snapshot.get('remote_ready'):
                state = intent()
                assert state['connection_requested'] is False
                services = adb(args.serial, 'shell', 'dumpsys', 'activity', 'services', PACKAGE)
                assert not any('ServiceRecord{' in line and '.ConnectionService' in line for line in services.splitlines())
                return
            time.sleep(.2)
        raise RuntimeError('Normal disconnect did not release the foreground service')

    def reconnect(log):
        nonlocal client
        if client:
            client.close()
        client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
            campaign / log, root / 'pab-mcp.exe')
        connected, raw = client.connect()
        assert connected.get('connected') is True and not raw.get('isError')

    try:
        home.start()
        if home.last_snapshot.get('paused'):
            home.click('resume')
        reconnect('mcp.log')
        tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
        tasks.fixture()
        before = tasks.count()
        text = '正式首页与通知停止验收'
        tasks.act('fixture_input', 'set_text', text=text)
        tasks.act('fixture_save')
        tasks.wait(text)
        assert tasks.count() == before + 1
        original = tasks.last_input_request
        result['original_request_id'] = original
        show_home()
        home.click('pause')
        assert intent()['paused'] is True
        blocked, _ = client.mobile(str(uuid.uuid4()), {'action':'navigate', 'destination':'home'})
        assert blocked['operation']['error'] == 'control_paused'
        assert not blocked['operation']['action_dispatched']
        result['checks']['normal_home_pause_persisted'] = True
        home.click('resume')
        assert intent()['paused'] is False
        # Only the fixed debug fixture receives FLAG_SECURE and a fake password input.
        # The ordinary service and authenticated MCP connection remain the owners.
        retained = phone_state(args.serial)['fingerprints']
        fixture = f'{PACKAGE}/com.pixels.agent.fixture.AutomationFixtureActivity'
        adb(args.serial, 'shell', 'am', 'start', '--activity-single-top', '--activity-clear-top',
            '-n', fixture, '--ez', 'secure_fixture', 'true')
        time.sleep(.8)
        try:
            deadline = time.monotonic() + 5
            while True:
                value, _ = client.mobile(str(uuid.uuid4()), {'action':'observe'})
                assert value['operation']['state'] == 'completed'
                screen = value['operation']['observation']
                protected = [node for node in screen['elements'] if node['protected']]
                if protected:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError('The dedicated protected window did not become visible')
                time.sleep(.2)
            assert len(protected) == 1 and protected[0].get('name') is None
            attempts = []
            for _ in range(4):
                observed, _ = client.mobile(str(uuid.uuid4()), {'action':'observe'})
                protected = [node for node in observed['operation']['observation']['elements'] if node['protected']]
                assert len(protected) == 1 and protected[0].get('name') is None
                request = str(uuid.uuid4())
                rejected, _ = client.mobile(request, {'action':'set_text',
                    'element_ref':protected[0]['element_ref'], 'text':'must-never-be-written'})
                operation = rejected['operation']
                attempts.append({'request_id':request, **{key:operation.get(key)
                    for key in ('state', 'error', 'action_dispatched')}})
                if operation.get('error') != 'target_stale' or operation['action_dispatched']:
                    break
                # Explicit pre-dispatch stale rejection permits a new observation.
                time.sleep(.3)
            result['protected_input_attempts'] = attempts
            assert operation['state'] == 'failed' and operation['error'] == 'unsupported_action', {
                key:operation.get(key) for key in ('state', 'error', 'action_dispatched')}
            assert not operation['action_dispatched']
            tasks.call({'action':'screenshot'}, expected_error='protected_content')
            result['checks']['secure_window_screenshot_rejected'] = True
            result['checks']['password_input_redacted_and_rejected'] = True
        finally:
            adb(args.serial, 'shell', 'am', 'start', '--activity-single-top', '--activity-clear-top',
                '-n', fixture, '--ez', 'secure_fixture', 'false', '--ez', 'reset_fixture', 'true')
            time.sleep(.5)
        capture, _ = tasks.call({'action':'screenshot'})
        assert capture['image']['width'] > 0 and capture['image']['height'] > 0
        result['checks']['screenshot_recovers_after_secure_window'] = True
        assert phone_state(args.serial)['fingerprints'] == retained
        show_home()
        home.tree()
        assert home.last_snapshot['notifications_enabled'] and home.last_snapshot['notification_stop_available']
        nonce = str(uuid.uuid4())
        adb(args.serial, 'shell', 'am', 'broadcast', '-n',
            f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
            '--es', 'production_request_id', nonce, '--es', 'probe_action', 'notification-stop')
        stopped()
        result['checks']['real_notification_pending_intent_disconnect'] = True
        assert home.start() is True
        reconnect('after-notification.log')
        old, _ = client.mobile(original, operation='get')
        assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
        result['checks']['original_request_after_notification_reconnect'] = True
        show_home()
        home.click('disconnect')
        stopped()
        result['checks']['normal_home_disconnect_releases_service'] = True
        assert home.start() is True
        result['checks']['normal_home_start'] = True
        reconnect('after-home.log')
        old, _ = client.mobile(original, operation='get')
        assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
        result['checks']['original_request_after_home_reconnect'] = True
        result['status'] = 'passed'
    finally:
        if client:
            client.close()
        (campaign / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'], 'checks':result['checks']}))


if __name__ == '__main__':
    main()
