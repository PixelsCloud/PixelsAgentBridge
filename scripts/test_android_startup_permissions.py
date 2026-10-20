#!/usr/bin/env python3
"""USB-assisted acceptance of ordinary local permission preparation; no permission grants via shell."""
from __future__ import annotations
import argparse
import json
import time
import uuid

from android_home import Home, LABELS, normal_status
from android_accessibility import set_enabled
from android_upgrade import phone_state
from test_android_remote import ROOT, PACKAGE, adb


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    parser.add_argument('--prepared', action='store_true', help='Verify a retained workspace choice from an earlier startup')
    parser.add_argument('--no-process-fault', action='store_true', help='Keep another pending system confirmation intact; do not test process restart')
    args = parser.parse_args()
    directory = ROOT / '.build/android/startup-permissions' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'checks': [], 'package_management_accepted': False,
              'android9_tested': False, 'permissions_revoked_via_adb': False}
    before = phone_state(args.serial)
    result['apk_sha256'] = before['apk_sha256']
    result['version_code'] = before['version_code']
    result['process_fault_tested'] = not args.no_process_fault
    home = Home(args.serial)
    def labels():
        return {node.get('text') for node in home.tree().iter('node') if node.get('text')}
    def wait_for(text, present=True):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if (text in labels()) == present: return
            time.sleep(.2)
        raise AssertionError('Unexpected startup permission screen: ' + text)
    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    try:
        adb(args.serial, 'shell', 'am', 'start', '-f', '0x14000000', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
        time.sleep(.7)
        names = labels()
        chinese = any(any('\u4e00' <= char <= '\u9fff' for char in name) for name in names)
        title = '准备应用权限' if chinese else 'Prepare app permissions'
        workspace = LABELS['permission_workspace'][0 if chinese else 1]
        ready = '当前设备和文件访问范围所需权限已准备好。' if chinese else 'Permissions are ready for the current device and file access choice.'
        def reveal_status():
            home.click('tab_access')
            # Home.start scrolls to the connection controls. The permission summary is
            # above them; absence outside the viewport does not mean a missing grant.
            for _ in range(6):
                current = labels()
                if ready in current or title in current: return
                adb(args.serial, 'shell', 'input', 'swipe', str(home.width * 9 // 10),
                    str(home.height * 4 // 5), str(home.width * 9 // 10), str(home.height // 4), '350')
                time.sleep(.5)
        reveal_status()
        # The physical development phone already has notification, accessibility and
        # install-source access. This first startup must request its missing directory choice.
        if args.prepared:
            wait_for(title, False); wait_for(ready)
            passed('startup_skips_existing_grants_and_saved_workspace_choice')
            home.click('permission_check'); wait_for(title); wait_for(workspace)
        else:
            wait_for(title); wait_for(workspace)
            assert normal_status(args.serial)['paused'] == before['paused']
            passed('startup_automatically_prepares_missing_permission_without_changing_pause')
        home.click('permission_workspace')
        wait_for(title, False); wait_for(ready)
        preferences = adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/permission-setup.toml')
        assert 'workspace_only = true' in preferences
        passed('workspace_choice_persisted_as_toml_and_already_granted_permissions_skipped')
        home.start(); home.set_paused(True)
        home.click('permission_check')
        wait_for(title); wait_for(workspace)
        home.click('permission_continue')
        deadline = time.monotonic() + 10
        while True:
            windows = adb(args.serial, 'shell', 'dumpsys', 'window')
            focused = next((line for line in windows.splitlines() if 'mCurrentFocus=' in line), '')
            if '.documentsui/' in focused: break
            assert time.monotonic() < deadline, 'Expected the normal Android directory picker'
            time.sleep(.2)
        for _ in range(10):
            adb(args.serial, 'shell', 'input', 'keyevent', '4')
            time.sleep(.3)
            focused = next((line for line in adb(args.serial, 'shell', 'dumpsys', 'window').splitlines() if 'mCurrentFocus=' in line), '')
            if '.documentsui/' not in focused: break
        wait_for(title, False); wait_for(ready)
        passed('startup_directory_picker_cancel_returns_without_repeated_prompts')
        home.click('permission_check')
        wait_for(title); wait_for(workspace)
        home.click('permission_later'); wait_for(title, False)
        passed('manual_retry_offers_directory_and_postpone_does_not_loop')
        home.click('disconnect')
        deadline = time.monotonic() + 10
        while normal_status(args.serial)['connection_requested']:
            assert time.monotonic() < deadline
            time.sleep(.2)
        home.click('permission_check'); wait_for(workspace)
        home.click('permission_workspace'); wait_for(title, False)
        stopped = normal_status(args.serial)
        assert not stopped['connection_requested'] and stopped['paused']
        passed('permission_preparation_does_not_start_a_stopped_connection_or_resume_control')
        home.start()
        if args.no_process_fault:
            after = phone_state(args.serial)
            assert all(after[key] == before[key] for key in ('apk_sha256', 'certificate_sha256', 'first_install_time', 'fingerprints', 'paused', 'connection_requested'))
            result['status'] = 'passed'
            return
        # Kill only this verified app process. Runtime lifecycle fault injection is not
        # a substitute for granting or checking permissions through the app/system UI.
        pid = normal_status(args.serial)['pid']
        assert adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', f'/proc/{pid}/cmdline').rstrip('\0') == PACKAGE
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'kill', '-9', str(pid))
        time.sleep(1)
        adb(args.serial, 'shell', 'am', 'start', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
        recovery = set_enabled(args.serial, True)
        home.start()
        assert normal_status(args.serial)['pid'] != pid
        reveal_status()
        wait_for(title, False); wait_for(ready)
        after = phone_state(args.serial)
        assert all(after[key] == before[key] for key in ('apk_sha256', 'certificate_sha256', 'first_install_time', 'fingerprints', 'paused', 'connection_requested'))
        passed('process_restart_preserves_choices_identity_and_control_state_without_reprompt', accessibility_recovery=recovery)
        result['status'] = 'passed'
    finally:
        result['ending_status'] = normal_status(args.serial)
        (directory / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'status': result['status'], 'checks': len(result['checks']), 'result': str(directory / 'result.json')}), flush=True)


if __name__ == '__main__':
    main()
