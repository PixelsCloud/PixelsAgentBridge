#!/usr/bin/env python3
"""Change only Pixels accessibility through the real system UI, including OEM consent.

Call after an explicit app launch when recovering a stopped process. Status broadcasts
must not be used to measure whether a force-stopped app restarts by itself.
"""
import argparse
import re
import time
import xml.etree.ElementTree as ET

from android_home import normal_status
from test_android_remote import PACKAGE, adb


def set_enabled(serial, enabled):
    if enabled and normal_status(serial)['accessibility_ready']:
        return {'system_ui_used':False, 'accessibility_ready':True}
    path = '/data/local/tmp/pab-accessibility-own.xml'

    def tree():
        try:
            adb(serial, 'shell', 'uiautomator', 'dump', path)
            return ET.fromstring(adb(serial, 'shell', 'cat', path))
        finally:
            adb(serial, 'shell', 'rm', '-f', path, check=False)

    def tap(node):
        left, top, right, bottom = map(int, re.findall(r'\d+', node.get('bounds', '')))
        assert right > left and bottom > top
        adb(serial, 'shell', 'input', 'tap', str((left + right) // 2), str((top + bottom) // 2))
        time.sleep(.3)

    def own(node):
        return node.get('text', '').startswith(
            ('Pixels Agent Bridge device control', 'Pixels device control', 'Pixels Android'))

    # MIUI may reuse an unrelated Settings child unless its test task is recreated.
    adb(serial, 'shell', 'am', 'start', '-f', '0x10008000', '-a', 'android.settings.ACCESSIBILITY_SETTINGS')
    time.sleep(.4)
    current = tree()
    downloaded = next((node for node in current.iter('node') if node.get('text') in
        ('已下载的应用', '已安装的服务', 'Downloaded apps', 'Installed services')), None)
    if downloaded is not None:
        tap(downloaded)
    for _ in range(15):
        current = tree()
        service = next((node for node in current.iter('node') if own(node)), None)
        if service is not None:
            tap(service)
            break
        # Scroll only inside the system accessibility service list. No other service is opened.
        geometry = next(current.iter('node')).get('bounds', '')
        _, _, width, height = map(int, re.findall(r'\d+', geometry))
        x = width * 9 // 10
        adb(serial, 'shell', 'input', 'swipe', str(x), str(height * 4 // 5),
            str(x), str(height // 4), '350')
        time.sleep(.6)
    else:
        raise RuntimeError('Own Pixels service was not found in the system service list')
    current = tree()
    assert any(own(node) for node in current.iter('node'))
    switch = next(node for node in current.iter('node')
        if node.get('resource-id', '').endswith(':id/switch_widget'))
    # An enabled-but-unbound OEM service needs a complete off/on transition.
    if switch.get('checked') == 'true':
        tap(switch)
        current = tree()
        stop = next((node for node in current.iter('node')
            if node.get('resource-id') == 'android:id/button1'), None)
        if stop is not None:
            assert any('Pixels' in node.get('text', '') for node in current.iter('node'))
            tap(stop)
        current = tree()
        switch = next(node for node in current.iter('node')
            if node.get('resource-id', '').endswith(':id/switch_widget'))
        assert switch.get('checked') == 'false'
    if enabled:
        tap(switch)
        deadline = time.monotonic() + 10
        checkbox = None
        allow = None
        while not normal_status(serial)['accessibility_ready']:
            current = tree()
            checkbox = next((node for node in current.iter('node')
                if node.get('resource-id') == 'com.miui.securitycenter:id/check_box'), None)
            allow = next((node for node in current.iter('node')
                if node.get('resource-id') == 'android:id/button1'), None)
            if checkbox is not None or allow is not None:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('The normal accessibility consent did not appear')
            time.sleep(.3)
        if checkbox is not None:
            if checkbox.get('checked') != 'true':
                tap(checkbox)
            deadline = time.monotonic() + 25
            while True:
                current = tree()
                allow = next((node for node in current.iter('node')
                    if node.get('resource-id') == 'com.miui.securitycenter:id/intercept_warn_allow'), None)
                if allow is None and normal_status(serial)['accessibility_ready']:
                    # The owner may finish the same ordinary consent while the
                    # snapshot is in flight. Never click a replacement page.
                    break
                if allow is not None and allow.get('enabled') == 'true':
                    tap(allow)
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError('The normal OEM consent countdown did not finish')
                time.sleep(.3)
        elif allow is not None:
            assert any('Pixels' in node.get('text', '') for node in current.iter('node'))
            tap(allow)
        # Never run another uiautomator session after enabling: it suppresses the service.
    adb(serial, 'shell', 'am', 'start', '--activity-reorder-to-front', '-n',
        f'{PACKAGE}/com.pixels.agent.MainActivity')
    deadline = time.monotonic() + 8
    while True:
        status = normal_status(serial)
        if status['accessibility_ready'] is enabled:
            return {'system_ui_used':True, 'accessibility_ready':enabled}
        if time.monotonic() >= deadline:
            raise RuntimeError('The Pixels service did not reach the requested binding state')
        time.sleep(.3)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['enable', 'disable'])
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    print(set_enabled(args.serial, args.action == 'enable'))
