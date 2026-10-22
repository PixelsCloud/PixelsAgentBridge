#!/usr/bin/env python3
"""Check the current Android home and tabs without logging visible credentials."""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import time
import tomllib
import uuid
import xml.etree.ElementTree as ET

from android_home import Home, LABELS, normal_status
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    args = parser.parse_args()
    directory = ROOT / '.build/android/product-refresh' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'checks': [], 'screenshots': []}
    resources = ROOT / 'apps/android/app/src/main/res'
    for folder in ('values-zh-rCN', 'values'):
        for element in ET.parse(resources / folder / 'product.xml').getroot():
            key = element.attrib['name']
            LABELS[key] = (*LABELS.get(key, ()), element.text)
    tabs = ['home', 'access', 'activity', 'settings']
    for tab in tabs:
        LABELS['tab_' + tab] = LABELS['product_tab_' + tab]
    home = Home(args.serial)

    def passed(name):
        result['checks'].append({'name': name, 'passed': True})
        print(json.dumps(result['checks'][-1]), flush=True)

    def fingerprints():
        return adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum',
                   'no_backup/settings.toml', 'no_backup/appearance.toml',
                   'no_backup/permission-setup.toml')

    def intent():
        return tomllib.loads(adb(args.serial, 'shell', 'run-as', PACKAGE,
                                'cat', 'no_backup/control-intent.toml'))

    def screenshot(name):
        names = {node.get('text') for node in home.tree().iter('node')}
        assert not names.intersection(LABELS['hide']), 'Hide the password before recording an image'
        path = directory / (name + '.png')
        path.write_bytes(subprocess.check_output([
            'adb', '-s', args.serial, 'exec-out', 'screencap', '-p']))
        result['screenshots'].append(path.name)

    def bounds(node):
        return list(map(int, re.findall(r'\d+', node.get('bounds'))))

    try:
        result['apk_sha256'] = installed_apk_sha256(args.serial)
        before_preferences = fingerprints()
        before_intent = intent()
        adb(args.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n',
            PACKAGE + '/com.pixels.agent.MainActivity')
        time.sleep(1)
        tree = home.tree()
        names = {node.get('text') for node in tree.iter('node')}
        assert 'Pixels Agent Bridge' in names
        assert names.intersection(LABELS['hide']) and not names.intersection(LABELS['show'])
        assert any(node.get('resource-id') == 'access_password' for node in tree.iter('node'))
        retired = set(LABELS['product_control_paused'] + LABELS['product_control_allowed'] +
                      LABELS['product_allow_control'] + LABELS['pause'] + LABELS['resume'])
        assert not names.intersection(retired)
        assert not any(node.get('resource-id') == 'control_status' for node in tree.iter('node'))
        assert not normal_status(args.serial)['paused']
        passed('full_app_name_visible_password_and_no_control_card')

        home.click('hide')
        names = {node.get('text') for node in home.tree().iter('node')}
        assert names.intersection(LABELS['show']) and not names.intersection(LABELS['hide'])
        screenshot('home')
        passed('password_can_still_be_hidden')

        for tab in tabs[1:]:
            home.click('tab_' + tab)
            time.sleep(.8)
            nodes = list(home.tree().iter('node'))
            selected = [node for node in nodes if node.get('selected') == 'true'
                        and bounds(node)[1] > home.height * .75]
            assert len(selected) == 1
            box = bounds(selected[0])
            assert any(node.get('text') in LABELS['tab_' + tab]
                       and box[0] <= bounds(node)[0] <= bounds(node)[2] <= box[2]
                       and box[1] <= bounds(node)[1] <= bounds(node)[3] <= box[3]
                       for node in nodes)
            assert any(node.get('text') == 'Pixels Agent Bridge' for node in nodes)
            screenshot(tab)
        passed('all_tabs_navigate_and_expose_exactly_one_selected_tab')

        # Inspect the pressed item animation without a background selection block.
        node = next(node for node in home.tree().iter('node')
                    if node.get('selected') == 'true' and bounds(node)[1] > home.height * .75)
        left, top, right, bottom = bounds(node)
        x, y = str((left + right) // 2), str((top + bottom) // 2)
        try:
            adb(args.serial, 'shell', 'input', 'motionevent', 'DOWN', x, y)
            time.sleep(.3)
            screenshot('settings-pressed')
        finally:
            adb(args.serial, 'shell', 'input', 'motionevent', 'UP', x, y)

        home.click('tab_home')
        time.sleep(.8)
        names = {node.get('text') for node in home.tree().iter('node')}
        assert names.intersection(LABELS['hide']) and not names.intersection(LABELS['show'])
        passed('returning_home_shows_password_by_default')

        status = normal_status(args.serial)
        assert not status['paused'] and not intent()['paused']
        assert intent()['connection_requested'] == before_intent['connection_requested']
        if status['remote_ready']:
            assert status['remote_control_allowed']
        passed('control_allowed_without_changing_connection_choice')
        assert fingerprints() == before_preferences
        passed('existing_configuration_preserved')
        result['status'] = 'passed'
    finally:
        (directory / 'result.json').write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(str(directory / 'result.json'), flush=True)


if __name__ == '__main__':
    main()
