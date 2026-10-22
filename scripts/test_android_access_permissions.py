#!/usr/bin/env python3
"""Verify unified Android permission rows and safe return from real system settings."""
from __future__ import annotations

import argparse
import json
import subprocess
import time
import uuid
import xml.etree.ElementTree as ET

from android_home import Home, LABELS, normal_status
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    args = parser.parse_args()
    directory = ROOT / '.build/android/access-permissions' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'checks': []}
    resources = ROOT / 'apps/android/app/src/main/res'
    for folder in ('values-zh-rCN', 'values'):
        for filename in ('product.xml', 'strings.xml'):
            for element in ET.parse(resources / folder / filename).getroot():
                key = element.attrib['name']
                LABELS[key] = (*LABELS.get(key, ()), element.text)
    LABELS['tab_access'] = LABELS['product_tab_access']
    home = Home(args.serial)

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps(result['checks'][-1]), flush=True)

    def names():
        return [node.get('text') for node in home.tree().iter('node') if node.get('text')]

    def focused():
        return next((line.strip() for line in adb(args.serial, 'shell', 'dumpsys', 'window')
                     .splitlines() if 'mCurrentFocus=' in line and PACKAGE + '/' in line), '') or next(
            (line.strip() for line in adb(args.serial, 'shell', 'dumpsys', 'window').splitlines()
             if 'mCurrentFocus=' in line and 'null' not in line), '')

    def wait_for(predicate):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            current = focused()
            if predicate(current):
                return current
            time.sleep(.2)
        raise AssertionError('Unexpected foreground window')

    def app_return():
        adb(args.serial, 'shell', 'input', 'keyevent', '4')
        time.sleep(.4)
        if PACKAGE + '/' not in focused():
            adb(args.serial, 'shell', 'am', 'start', '--activity-reorder-to-front', '-n',
                PACKAGE + '/com.pixels.agent.MainActivity')
        wait_for(lambda value: PACKAGE + '/' in value)
        time.sleep(.8)
        assert not set(names()).intersection(LABELS['permission_setup_title']), \
            'A manual permission row must not restart the startup authorization sequence'

    def fingerprints():
        return adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum',
                   'no_backup/settings.toml', 'no_backup/control-intent.toml',
                   'no_backup/appearance.toml', 'no_backup/permission-setup.toml')

    def capture(name):
        (directory / (name + '.png')).write_bytes(subprocess.check_output([
            'adb', '-s', args.serial, 'exec-out', 'screencap', '-p']))

    try:
        result['apk_sha256'] = installed_apk_sha256(args.serial)
        before = fingerprints()
        adb(args.serial, 'shell', 'am', 'start', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
        time.sleep(.8)
        home.click('tab_access')
        time.sleep(.8)
        current = names()
        for key in ('product_app_permissions', 'product_interface_control',
                    'product_app_notifications', 'product_notification_access',
                    'product_install_apps', 'product_folders', 'product_add_folder'):
            assert sum(value in LABELS[key] for value in current) == 1, key
        for key in ('product_access_title', 'product_access_subtitle', 'product_access_scope_hint',
                    'permission_setup_ready', 'permission_setup_missing',
                    'package_install_settings', 'permission_setup_check'):
            assert not set(current).intersection(LABELS[key]), key
        sdk = int(adb(args.serial, 'shell', 'getprop', 'ro.build.version.sdk').strip())
        if sdk >= 30:
            assert not set(current).intersection(LABELS['product_screen_view'])
        passed('permissions_and_folder_management_are_unified_without_repeated_hints')
        granted_before = sum(value in LABELS['product_permission_granted'] for value in current)
        assert granted_before == (4 if sdk >= 30 else 5), 'Phone must retain its existing grants'
        assert not set(current).intersection(LABELS['product_grant_all'])
        capture('access')
        pid = adb(args.serial, 'shell', 'pidof', PACKAGE).strip()

        for key, action, data in (
            ('product_interface_control', 'android.settings.ACCESSIBILITY_SETTINGS', []),
            ('product_app_notifications', 'android.settings.APP_NOTIFICATION_SETTINGS',
             ['--es', 'android.provider.extra.APP_PACKAGE', PACKAGE]),
            ('product_notification_access', 'android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS', []),
            ('product_install_apps', 'android.settings.MANAGE_UNKNOWN_APP_SOURCES',
             ['-d', 'package:' + PACKAGE]),
        ):
            resolved = adb(args.serial, 'shell', 'cmd', 'package', 'resolve-activity',
                           '--brief', '-a', action, *data).strip().splitlines()[-1]
            assert '/' in resolved, 'Missing system handler: ' + action
            target = resolved.split('/')[0]
            home.click(key)
            target_window = wait_for(lambda value: f' {target}/' in value)
            assert adb(args.serial, 'shell', 'pidof', PACKAGE).strip() == pid
            app_return()
            assert sum(value in LABELS['product_permission_granted'] for value in names()) == granted_before
            passed(key + '_opens_system_settings_and_returns_without_extra_prompts',
                   target_window=target_window)

        home.click('product_add_folder')
        wait_for(lambda value: '.documentsui/' in value)
        for _ in range(8):
            adb(args.serial, 'shell', 'input', 'keyevent', '4')
            time.sleep(.4)
            if '.documentsui/' not in focused():
                break
        wait_for(lambda value: PACKAGE + '/' in value)
        assert not set(names()).intersection(LABELS['files_grant_failed'])
        passed('folder_picker_cancel_retains_access')
        assert fingerprints() == before
        status = normal_status(args.serial)
        assert not status['paused']
        passed('preferences_and_control_state_preserved')
        capture('access-final')
        result['status'] = 'passed'
    finally:
        (directory / 'result.json').write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(str(directory / 'result.json'), flush=True)


if __name__ == '__main__':
    main()
