#!/usr/bin/env python3
"""Inject unreadable/future settings on a disposable emulator and verify safe recovery."""
import argparse
import hashlib
import json
import time
import uuid
import xml.etree.ElementTree as ET

from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    assert args.serial.startswith('emulator-'), 'This fault injection is limited to the disposable emulator'
    run = uuid.uuid4()
    root = ROOT / '.build/android/settings-failure' / str(run)
    root.mkdir(parents=True)
    digest = installed_apk_sha256(args.serial)
    original = adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml')
    backup = f'files/settings-check-{run}.toml'
    staged = f'/data/local/tmp/pab-settings-check-{run}.toml'
    window = f'/data/local/tmp/pab-settings-window-{run}.xml'
    result = {'status':'failed', 'provenance':{'apk_sha256':digest, 'serial':args.serial},
        'scope':'unreadable and unsupported settings preserve source document; recovery restores original bytes',
        'cases':[]}
    adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', 'no_backup/settings.toml', backup)

    def reopen():
        adb(args.serial, 'shell', 'am', 'force-stop', PACKAGE)
        adb(args.serial, 'shell', 'am', 'start', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
        time.sleep(1)

    def screen_has_error():
        adb(args.serial, 'shell', 'uiautomator', 'dump', window)
        screen = ET.fromstring(adb(args.serial, 'shell', 'cat', window))
        return any('原设置已保留' in node.get('text', '') or 'settings have been preserved' in node.get('text', '')
            for node in screen.iter('node') if node.get('package') == PACKAGE)

    try:
        for name, content in (('malformed_toml', 'schema_version = [\n'),
                              ('unsupported_future_version', 'schema_version = 2147483647\n')):
            file = root / 'invalid-settings.toml'
            file.write_text(content, encoding='utf-8', newline='\n')
            adb(args.serial, 'push', str(file), staged)
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', staged, 'no_backup/settings.toml')
            reopen()
            assert screen_has_error(), 'The normal home did not show preserved-settings recovery'
            retained = adb(args.serial, 'shell', 'run-as', PACKAGE,
                'sha256sum', 'no_backup/settings.toml').split()[0]
            assert retained == hashlib.sha256(file.read_bytes()).hexdigest(), 'Unreadable configuration was overwritten'
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', backup, 'no_backup/settings.toml')
            reopen()
            assert not screen_has_error(), 'Original valid configuration could not be restored'
            assert adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml') == original
            result['cases'].append({'name':name, 'unreadable_bytes_preserved':True,
                'normal_recovery_message':True, 'original_config_restored':True})
        assert installed_apk_sha256(args.serial) == digest
        result['status'] = 'passed'
    finally:
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', backup, 'no_backup/settings.toml')
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', backup)
        adb(args.serial, 'shell', 'rm', '-f', staged, window)
        reopen()
        (root / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps(result))


if __name__ == '__main__':
    main()
