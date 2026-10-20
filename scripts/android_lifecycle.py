#!/usr/bin/env python3
"""Measure Pixels process/permission boundaries, preserving normal identity and pause.

SIGKILL is explicit fault injection, not a claim that Android selected the process
under memory pressure. Force-stop observations never send app broadcasts until the
explicit foreground launch. No other package is stopped or changed.
"""
from __future__ import annotations

import argparse
import json
import time
import uuid

from android_accessibility import set_enabled
from android_home import Home, normal_status
from android_public import PublicMcp
from android_upgrade import phone_state
from test_android_remote import ROOT, PACKAGE, adb


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    args = parser.parse_args()
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    original = json.loads((root / 'lifecycle-baseline-v14.json').read_text(encoding='utf-8'))
    home = Home(args.serial)
    home.start()
    home.set_paused(True)
    baseline = phone_state(args.serial)
    assert baseline['paused'] is True and baseline['connection_requested'] is True
    directory = root / ('lifecycle-' + str(uuid.uuid4()))
    directory.mkdir()
    report = {'status':'failed', 'provenance':{'apk_sha256':baseline['apk_sha256'],
        'serial':args.serial, 'public_server':True}, 'cases':[]}

    def verify(label):
        current = phone_state(args.serial)
        for key in ('certificate_sha256', 'first_install_time', 'fingerprints',
                    'paused', 'connection_requested'):
            assert current[key] == baseline[key], key
        client = PublicMcp(root / 'mcp.toml', '919383159', True,
            directory / (label + '.log'), root / 'pab-mcp.exe')
        try:
            connected, raw = client.connect()
            assert connected['connected'] and not raw.get('isError')
            expected = json.loads((root / 'upgrade-baseline-after-rotation.json').read_text())['device_ref']
            assert connected['device_ref'] == expected
            old, _ = client.mobile(original['original_request_id'], operation='get')
            assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
            blocked, _ = client.mobile(str(uuid.uuid4()), {'action':'navigate', 'destination':'home'})
            assert blocked['operation']['error'] == 'control_paused'
            assert not blocked['operation']['action_dispatched']
        finally:
            client.close()
        report['cases'].append({'name':label, 'status':'passed',
            'identity_credentials_settings_pause_retained':True,
            'original_request_query':'completed', 'paused_input_not_dispatched':True})
        (directory / 'progress.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        print('Lifecycle boundary passed: ' + label, flush=True)

    try:
        old_pid = adb(args.serial, 'shell', 'pidof', PACKAGE).strip()
        assert old_pid.isdigit()
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'kill', '-9', old_pid)
        observed = []
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            pid = adb(args.serial, 'shell', 'pidof', PACKAGE, check=False).strip()
            observed.append({'elapsed_seconds':round(20 - (deadline - time.monotonic()), 2),
                'pid':int(pid) if pid.isdigit() else None})
            if pid and pid != old_pid:
                break
            time.sleep(.5)
        assert any(row['pid'] != int(old_pid) for row in observed)
        report['sigkill'] = {'previous_pid':int(old_pid), 'observations':observed,
            'automatic_restart_observed':bool(pid and pid != old_pid), 'low_memory_selection_tested':False}
        # Explicit recovery starts from the same normal Activity a user can open.
        adb(args.serial, 'shell', 'am', 'start', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
        report['sigkill']['accessibility_recovery'] = set_enabled(args.serial, True)
        home.start()
        verify('process_sigkill_recovery')

        adb(args.serial, 'shell', 'am', 'force-stop', PACKAGE)
        stopped = []
        started = time.monotonic()
        while time.monotonic() - started < 15:
            pid = adb(args.serial, 'shell', 'pidof', PACKAGE, check=False).strip()
            assert not pid, 'A force-stopped app unexpectedly restarted'
            stopped.append({'elapsed_seconds':round(time.monotonic() - started, 2), 'process_present':False})
            time.sleep(1)
        report['force_stop'] = {'passive_observations':stopped, 'automatic_restart_claimed':False}
        adb(args.serial, 'shell', 'am', 'start', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
        report['force_stop']['accessibility_recovery'] = set_enabled(args.serial, True)
        home.start()
        verify('force_stop_foreground_restart')

        home.set_paused(False)
        report['permission_revocation'] = set_enabled(args.serial, False)
        client = PublicMcp(root / 'mcp.toml', '919383159', True,
            directory / 'permission-denied.log', root / 'pab-mcp.exe')
        try:
            connected, raw = client.connect()
            assert connected['connected'] and not raw.get('isError')
            failures = []
            for command in ({'action':'observe'}, {'action':'navigate', 'destination':'home'}):
                value, _ = client.mobile(str(uuid.uuid4()), command)
                operation = value['operation']
                assert operation['state'] == 'failed' and operation.get('error') == 'service_unavailable'
                assert not operation['action_dispatched']
                failures.append({'action':command['action'], 'error':operation['error'], 'action_dispatched':False})
            report['permission_denials'] = failures
        finally:
            client.close()
        report['permission_regrant'] = set_enabled(args.serial, True)
        home.set_paused(True)
        verify('permission_regrant')
        report['status'] = 'passed'
    finally:
        report['observed_at_unix'] = time.time()
        (directory / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        print(json.dumps({'directory':str(directory), 'status':report['status']}), flush=True)


if __name__ == '__main__':
    main()
