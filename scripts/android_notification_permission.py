#!/usr/bin/env python3
"""Verify an explicit notification denial still permits the ordinary foreground service."""
import argparse
import json
import re
import time
import uuid

from android_home import Home, normal_status
from android_accessibility import set_enabled
from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256

PERMISSION = 'android.permission.POST_NOTIFICATIONS'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    args = parser.parse_args()
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert installed_apk_sha256(args.serial) == session['apk_sha256']
    package = adb(args.serial, 'shell', 'dumpsys', 'package', PACKAGE)
    permission = re.search(r'android\.permission\.POST_NOTIFICATIONS: granted=(\w+), flags=\[([^\]]*)\]', package)
    assert permission and permission[1] == 'true', 'Begin with the existing granted notification permission'
    flags = permission[2].split('|')
    assert not any('FIXED' in flag and flag.strip() != 'USER_FIXED' for flag in flags)
    restored_flags = [name for name, label in (('user-set', 'USER_SET'), ('user-fixed', 'USER_FIXED'))
        if label in {flag.strip() for flag in flags}]
    campaign = root / ('notification-permission-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True}, 'checks':{}}
    home = Home(args.serial)
    client = None

    def wait(ready):
        deadline = time.monotonic() + 55
        while True:
            status = normal_status(args.serial)
            if status['remote_ready'] is ready and (ready or status['phase'] == 'DISCONNECTED'):
                return status
            if time.monotonic() >= deadline:
                raise RuntimeError('Normal foreground service did not reach the requested state')
            time.sleep(.3)

    def restore_permission():
        adb(args.serial, 'shell', 'pm', 'grant', PACKAGE, PERMISSION)
        adb(args.serial, 'shell', 'pm', 'clear-permission-flags', PACKAGE, PERMISSION, 'user-set', 'user-fixed')
        if restored_flags:
            adb(args.serial, 'shell', 'pm', 'set-permission-flags', PACKAGE, PERMISSION, *restored_flags)

    try:
        print('Notification test: starting ordinary connection', flush=True)
        home.start()
        if home.last_snapshot.get('paused'):
            home.click('resume')
        home.click('disconnect')
        wait(False)
        print('Notification test: injecting explicit denial', flush=True)
        adb(args.serial, 'shell', 'pm', 'revoke', PACKAGE, PERMISSION)
        adb(args.serial, 'shell', 'pm', 'set-permission-flags', PACKAGE, PERMISSION, 'user-set', 'user-fixed')
        adb(args.serial, 'shell', 'am', 'start', '--activity-reorder-to-front', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        result['notification_revoke_service_recovery'] = set_enabled(args.serial, True)
        time.sleep(.8)  # Let the real Main Activity finish its return transition.
        # The real Activity permission callback receives denial. Never press Allow.
        home.click('connect')
        time.sleep(.8)
        status = normal_status(args.serial)
        if status['phase'] == 'DISCONNECTED' and not status['connection_requested']:
            # No connection intent was accepted; resolve the current UI button again.
            home.click('connect')
        denied = wait(True)
        print('Notification test: foreground connection ready with denial', flush=True)
        assert denied['notifications_enabled'] is False
        services = adb(args.serial, 'shell', 'dumpsys', 'activity', 'services', PACKAGE)
        assert 'isForeground=true' in services
        client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
            campaign / 'mcp.log', root / 'pab-mcp.exe')
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
        tasks.fixture()
        before = tasks.count()
        text = '通知拒绝后的真实操作'
        tasks.act('fixture_input', 'set_text', text=text)
        tasks.act('fixture_save')
        tasks.wait(text)
        assert tasks.count() == before + 1
        original = tasks.last_input_request
        result['checks']['denied_notification_real_foreground_remote_input'] = True
        adb(args.serial, 'shell', 'am', 'start', '--activity-reorder-to-front', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        home.click('disconnect')
        wait(False)
        result['checks']['denied_notification_home_disconnect'] = True
        restore_permission()
        assert home.start() is True
        granted = wait(True)
        assert granted['notifications_enabled'] is True
        client.close()
        client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
            campaign / 'after-grant.log', root / 'pab-mcp.exe')
        connected, raw = client.connect()
        assert connected['connected'] and not raw.get('isError')
        previous, _ = client.mobile(original, operation='get')
        assert previous['operation']['state'] == 'completed' and previous['operation']['action_dispatched']
        result['checks'].update(granted_notification_connection=True, original_request_retained=True,
            original_permission_and_user_flags_restored=True)
        result['status'] = 'passed'
    finally:
        restore_permission()
        if client:
            client.close()
        (campaign / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'], 'checks':result['checks']}))


if __name__ == '__main__':
    main()
