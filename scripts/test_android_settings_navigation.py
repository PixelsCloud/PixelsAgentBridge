#!/usr/bin/env python3
"""Verify settings links on the USB phone without changing settings or app data."""
from __future__ import annotations

import argparse
import json
import re
import time
import uuid
import xml.etree.ElementTree as ET

from android_home import Home, LABELS
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    args = parser.parse_args()
    directory = ROOT / '.build/android/settings-navigation' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'checks': []}
    resources = ROOT / 'apps/android/app/src/main/res'
    for folder in ('values-zh-rCN', 'values'):
        for filename in ('product.xml', 'strings.xml'):
            for element in ET.parse(resources / folder / filename).getroot():
                key = element.attrib.get('name')
                if key in ('product_tab_settings', 'startup_app_settings', 'startup_battery_settings'):
                    LABELS[key] = (*LABELS.get(key, ()), element.text)
    LABELS['tab_settings'] = LABELS['product_tab_settings']
    home = Home(args.serial)

    def focused_window():
        return next((line for line in adb(args.serial, 'shell', 'dumpsys', 'window').splitlines()
                     if 'mCurrentFocus=' in line), '')

    def wait_for_package(package, alternative_component=None):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            focused = focused_window()
            if (f' {package}/' in focused or
                    alternative_component and f' {alternative_component}' in focused):
                return focused.strip()
            time.sleep(.2)
        raise AssertionError('Expected foreground package: ' + package)

    def fingerprints():
        return adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum',
                   'no_backup/settings.toml', 'no_backup/control-intent.toml',
                   'no_backup/appearance.toml', 'no_backup/permission-setup.toml')

    def crash_log():
        return adb(args.serial, 'logcat', '-b', 'crash', '-d', '-v', 'threadtime')

    def crash_records(log):
        return [line for line in log.splitlines() if f'Process: {PACKAGE},' in line
                or f'>>> {PACKAGE} <<<' in line]

    def passed(name, **metadata):
        result['checks'].append({'name': name, 'passed': True, **metadata})
        print(json.dumps(result['checks'][-1], ensure_ascii=False), flush=True)

    try:
        result['apk_sha256'] = installed_apk_sha256(args.serial)
        package_info = adb(args.serial, 'shell', 'dumpsys', 'package', PACKAGE)
        result['version_code'] = int(re.search(r'versionCode=(\d+)', package_info)[1])
        before_preferences = fingerprints()
        before_crashes = crash_records(crash_log())
        adb(args.serial, 'shell', 'am', 'start', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
        wait_for_package(PACKAGE)
        time.sleep(.7)
        pid = adb(args.serial, 'shell', 'pidof', PACKAGE).strip()
        assert pid, 'Pixels must be running before opening settings'
        home.click('tab_settings')
        time.sleep(.8)
        links = (
            ('startup_app_settings', 'android.settings.APPLICATION_DETAILS_SETTINGS',
             ['-d', 'package:' + PACKAGE]),
            ('startup_battery_settings', 'android.settings.IGNORE_BATTERY_OPTIMIZATION_SETTINGS', []),
        )
        for attempt in range(2):
            for key, action, data in links:
                resolved = adb(args.serial, 'shell', 'cmd', 'package', 'resolve-activity',
                               '--brief', '-a', action, *data).strip().splitlines()[-1]
                assert '/' in resolved, 'System settings handler unavailable for ' + action
                target_package = resolved.split('/')[0]
                assert target_package != PACKAGE
                home.click(key)
                time.sleep(.8)
                if f' {PACKAGE}/' in focused_window():
                    # A tap during the return transition can only cancel a fling.
                    home.click(key)
                # MIUI's AOSP app-details handler redirects to its app manager.
                redirect = ('com.miui.securitycenter/com.miui.appmanager.ApplicationsDetailsActivity'
                            if key == 'startup_app_settings' and target_package == 'com.android.settings'
                            else None)
                target_window = wait_for_package(target_package, redirect)
                assert adb(args.serial, 'shell', 'pidof', PACKAGE).strip() == pid, \
                    'Pixels process died while opening system settings'
                adb(args.serial, 'shell', 'input', 'keyevent', '4')
                time.sleep(.4)
                # NEW_TASK may reuse an existing Settings task with older pages.
                # Selecting Pixels again must reuse its surviving app process.
                if f' {PACKAGE}/' not in focused_window():
                    adb(args.serial, 'shell', 'am', 'start', '-n',
                        PACKAGE + '/com.pixels.agent.MainActivity')
                wait_for_package(PACKAGE)
                time.sleep(.8)
                assert adb(args.serial, 'shell', 'pidof', PACKAGE).strip() == pid, \
                    'Pixels process restarted when returning from system settings'
                passed(key, attempt=attempt + 1, target_window=target_window,
                       process_survived=True)
        after_log = crash_log()
        (directory / 'crash-after.log').write_text(after_log, encoding='utf-8')
        new_crashes = [record for record in crash_records(after_log) if record not in before_crashes]
        assert not new_crashes, 'New Pixels crashes were recorded during settings navigation'
        passed('no_new_crashes')
        assert fingerprints() == before_preferences, 'Settings navigation changed app preferences'
        passed('preferences_preserved')
        result['status'] = 'passed'
    finally:
        (directory / 'result.json').write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(str(directory), flush=True)


if __name__ == '__main__':
    main()
