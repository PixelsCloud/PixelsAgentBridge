#!/usr/bin/env python3
"""Check fixed activity filters while scrolling actual device history."""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import time
import uuid
import xml.etree.ElementTree as ET

from android_home import Home, LABELS
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    args = parser.parse_args()
    directory = ROOT / '.build/android/activity-tabs' / str(uuid.uuid4())
    directory.mkdir(parents=True)
    result = {'status': 'failed', 'serial': args.serial, 'checks': []}
    resources = ROOT / 'apps/android/app/src/main/res'
    for folder in ('values-zh-rCN', 'values'):
        for element in ET.parse(resources / folder / 'product.xml').getroot():
            key = element.attrib['name']
            LABELS[key] = (*LABELS.get(key, ()), element.text)
    LABELS['tab_activity'] = LABELS['product_tab_activity']
    home = Home(args.serial)

    def passed(name):
        result['checks'].append({'name': name, 'passed': True})
        print(json.dumps(result['checks'][-1]), flush=True)

    def nodes():
        return list(home.tree().iter('node'))

    def tagged(values, identifier):
        matches = [node for node in values if node.get('resource-id') == identifier]
        assert len(matches) == 1, 'Missing or duplicated control: ' + identifier
        return matches[0]

    def bounds(node):
        return tuple(map(int, re.findall(r'\d+', node.get('bounds'))))

    def tab_bounds(values):
        return [bounds(tagged(values, 'activity_filter_' + str(index))) for index in range(3)]

    def list_text(values):
        top = bounds(tagged(values, 'activity_list'))[1]
        return [node.get('text') for node in values if node.get('text') and bounds(node)[1] >= top
                and bounds(node)[3] <= bounds(tagged(values, 'activity_list'))[3]]

    def capture(name):
        (directory / (name + '.png')).write_bytes(subprocess.check_output([
            'adb', '-s', args.serial, 'exec-out', 'screencap', '-p']))

    def fingerprints():
        return adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum',
                   'no_backup/settings.toml', 'no_backup/control-intent.toml',
                   'no_backup/appearance.toml', 'no_backup/permission-setup.toml')

    def select(index):
        target = tagged(nodes(), 'activity_filter_' + str(index))
        left, top, right, bottom = bounds(target)
        adb(args.serial, 'shell', 'input', 'tap', str((left + right) // 2),
            str((top + bottom) // 2))
        time.sleep(.8)
        current = nodes()
        assert tagged(current, 'activity_filter_' + str(index)).get('selected') == 'true'
        assert sum(tagged(current, 'activity_filter_' + str(item)).get('selected') == 'true'
                   for item in range(3)) == 1
        return current

    try:
        result['apk_sha256'] = installed_apk_sha256(args.serial)
        before = fingerprints()
        adb(args.serial, 'shell', 'am', 'start', '-n', PACKAGE + '/com.pixels.agent.MainActivity')
        time.sleep(.8)
        home.click('tab_activity')
        time.sleep(.8)
        current = select(0)
        assert sum(node.get('text') in LABELS['product_tab_activity'] for node in current) == 1, \
            'The activity page must only show Activity in the bottom navigation'
        fixed = tab_bounds(current)
        passed('heading_removed_and_three_filters_available')
        capture('activity-all')

        initial_content = [(node.get('text'), node.get('bounds')) for node in current
                           if node.get('text') in list_text(current)]
        left, top, right, bottom = bounds(tagged(current, 'activity_list'))
        for _ in range(3):
            adb(args.serial, 'shell', 'input', 'swipe', str((left + right) // 2),
                str(top + (bottom - top) * 4 // 5), str((left + right) // 2),
                str(top + (bottom - top) // 5), '400')
            time.sleep(.7)
            current = nodes()
            assert tab_bounds(current) == fixed, 'Activity filters scrolled away with the list'
        final_content = [(node.get('text'), node.get('bounds')) for node in current
                         if node.get('text') in list_text(current)]
        assert final_content != initial_content, 'The history list did not scroll'
        passed('history_scrolls_while_filter_positions_stay_fixed')
        capture('activity-scrolled')

        current = select(1)
        assert tab_bounds(current) == fixed
        content = list_text(current)
        assert any(text in LABELS['product_completed'] for text in content)
        for key in ('product_activity_failed', 'product_unsupported', 'product_activity_pending',
                    'product_activity_dispatched'):
            assert not set(content).intersection(LABELS[key])
        passed('completed_filter_still_works_after_scrolling')
        capture('activity-completed')

        current = select(2)
        assert tab_bounds(current) == fixed
        assert not set(list_text(current)).intersection(LABELS['product_completed'])
        passed('incomplete_filter_excludes_completed_records')
        capture('activity-incomplete')
        select(0)
        assert fingerprints() == before
        passed('existing_configuration_preserved')
        result['status'] = 'passed'
    finally:
        (directory / 'result.json').write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
        print(str(directory / 'result.json'), flush=True)


if __name__ == '__main__':
    main()
