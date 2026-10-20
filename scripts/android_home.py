#!/usr/bin/env python3
"""Drive the app's ordinary home controls for USB-assisted setup and upgrade checks.

This is not remote-control acceptance. Credentials stay in memory and travel only
to the existing pairing CLI's stdin. No screen dumps are kept.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
import tomllib
import uuid
import xml.etree.ElementTree as ET

from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256

LABELS = {
    'tab_home':('首页', 'Home'),
    'tab_access':('访问', 'Access'),
    'permission_workspace':('仅使用工作目录', 'Use workspace only'),
    'permission_check':('检查应用权限', 'Check app permissions'),
    'permission_continue':('授权并继续', 'Authorize and continue'),
    'permission_later':('稍后设置', 'Set up later'),
    'install_settings':('管理安装应用权限', 'Manage permission to install apps'),
    'connect':('保存并连接', 'Save and connect'),
    'disconnect':('断开连接', 'Disconnect'),
    'pause':('暂停控制', 'Pause control'),
    'resume':('恢复控制', 'Resume control', '允许控制', 'Allow control'),
    'show':('显示密码', 'Show password'),
    'hide':('隐藏密码', 'Hide password'),
    'rotate':('重置访问密码', 'Reset access password'),
    'reload':('重新读取设置', 'Reload settings'),
}


class Home:
    def __init__(self, serial):
        self.serial = serial
        size = adb(serial, 'shell', 'wm', 'size')
        self.width, self.height = map(int, re.findall(r'(\d+)x(\d+)', size)[-1])

    def tree(self):
        request = str(uuid.uuid4())
        adb(self.serial, 'shell', 'am', 'broadcast', '-n',
            f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
            '--es', 'production_request_id', request, '--es', 'probe_action', 'home-snapshot')
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                value = json.loads(adb(self.serial, 'shell', 'run-as', PACKAGE, 'cat', 'files/home-snapshot.json'))
            except (ValueError, subprocess.CalledProcessError):
                time.sleep(.1)
                continue
            if value.get('request_id') == request:
                if value.get('error'):
                    time.sleep(.2)
                    adb(self.serial, 'shell', 'am', 'broadcast', '-n',
                        f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
                        '--es', 'production_request_id', request, '--es', 'probe_action', 'home-snapshot')
                    continue
                self.last_snapshot = value
                tree = ET.Element('hierarchy')
                for node in value['elements']:
                    ET.SubElement(tree, 'node', {
                        'text':node['text'], 'class':node['role'], 'package':PACKAGE,
                        'clickable':str(node['clickable']).lower(), 'enabled':str(node['enabled']).lower(),
                        'selected':str(node.get('selected', False)).lower(),
                        'resource-id':node.get('identifier', ''),
                        'bounds':f"[{node['left']},{node['top']}][{node['right']},{node['bottom']}]"})
                return tree
            time.sleep(.1)
        raise RuntimeError('The debug home snapshot did not finish for this request')

    def credential_tree(self):
        # Used only after connection, to read the user-revealed password into memory.
        # No input is dispatched from this transient uiautomator snapshot.
        path = f'/data/local/tmp/pab-home-{uuid.uuid4()}.xml'
        try:
            adb(self.serial, 'shell', 'uiautomator', 'dump', path)
            return ET.fromstring(adb(self.serial, 'shell', 'cat', path))
        finally:
            adb(self.serial, 'shell', 'rm', '-f', path, check=False)

    def set_paused(self, paused):
        """Wait for the normal toggle's persisted outcome before recording evidence."""
        adb(self.serial, 'shell', 'am', 'start', '-f', '0x14000000', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        time.sleep(.6)
        for _ in range(3):
            if normal_status(self.serial)['paused'] is paused:
                return
            self.click('pause' if paused else 'resume')
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                if normal_status(self.serial)['paused'] is paused:
                    intent = tomllib.loads(adb(self.serial, 'shell', 'run-as', PACKAGE,
                        'cat', 'no_backup/control-intent.toml'))
                    if intent['paused'] is paused:
                        return
                time.sleep(.2)
        raise RuntimeError('The normal control pause toggle did not reach its persisted state')

    def click(self, key, *, already=None):
        tab = ('tab_home' if key in ('connect', 'disconnect', 'pause', 'resume', 'show', 'hide', 'rotate')
               else 'tab_access' if key in ('permission_check', 'install_settings') else None)
        if tab:
            tree = self.tree()
            node = next((item for item in tree.iter('node') if item.get('text') in LABELS[tab]), None)
            if node is not None and node.get('selected') != 'true':
                self.click(tab)
        for direction in (1, -1):
            for _ in range(10):
                tree = self.tree()
                if already and any(item.get('text') in LABELS[already] for item in tree.iter('node')):
                    return False
                parents = {child:parent for parent in tree.iter() for child in parent}
                node = next((item for item in tree.iter('node')
                    if item.get('text') in LABELS[key] and item.get('package') == PACKAGE), None)
                if node is not None:
                    label = node
                    while node is not None and node.get('clickable') != 'true':
                        node = parents.get(node)
                    if node is None:
                        left, top, right, bottom = map(int, re.findall(r'\d+', label.get('bounds', '')))
                        candidates = [item for item in tree.iter('node') if item.get('clickable') == 'true'
                            and item.get('enabled') == 'true'
                            and (lambda box:box[0] <= left and box[1] <= top and box[2] >= right and box[3] >= bottom)(
                                list(map(int, re.findall(r'\d+', item.get('bounds', '')))))]
                        if len(candidates) == 1:
                            node = candidates[0]
                        elif key.startswith('tab_'):
                            # A selected Compose tab has no click action. Tapping its
                            # visible label is harmless and keeps setup navigation idempotent.
                            node = label
                    if node is not None and node.get('enabled') == 'true':
                        left, top, right, bottom = map(int, re.findall(r'\d+', node.get('bounds', '')))
                        x, y = (left + right) // 2, (top + bottom) // 2
                        if 0 < x < self.width and 0 < y < self.height - 80:
                            adb(self.serial, 'shell', 'input', 'tap', str(x), str(y))
                            self.last_click = {'control':key, 'x':x, 'y':y, 'bounds':node.get('bounds')}
                            time.sleep(.2)
                            return True
                start, end = (.8, .25) if direction == 1 else (.25, .8)
                adb(self.serial, 'shell', 'input', 'swipe', str(self.width * 9 // 10),
                    str(int(self.height * start)), str(self.width * 9 // 10), str(int(self.height * end)), '300')
                time.sleep(.7)  # A tap during a Compose fling only cancels scrolling.
        raise RuntimeError(f'The ordinary home control is unavailable: {key}')

    def start(self):
        adb(self.serial, 'shell', 'am', 'start', '-f', '0x14000000', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        time.sleep(.5)
        pressed = self.click('connect', already='disconnect')
        time.sleep(.5)
        # Notification consent uses the system's own dialog; normal home snapshots
        # intentionally refuse to expose another package's UI.
        if pressed and not self.last_snapshot.get('notifications_enabled'):
            tree = self.credential_tree()
            allow = next((item for item in tree.iter('node') if
                item.get('resource-id', '').endswith(':id/permission_allow_button')), None)
            if allow is not None:
                left, top, right, bottom = map(int, re.findall(r'\d+', allow.get('bounds', '')))
                adb(self.serial, 'shell', 'input', 'tap', str((left + right) // 2), str((top + bottom) // 2))
        deadline = time.monotonic() + 55
        retry_after = time.monotonic() + 3
        retried = False
        while time.monotonic() < deadline:
            self.tree()
            if self.last_snapshot.get('remote_ready'):
                return pressed
            if self.last_snapshot.get('phase') == 'FAILED':
                break
            if (not retried and time.monotonic() >= retry_after
                    and self.last_snapshot.get('phase') == 'DISCONNECTED'):
                # An Activity entering after the OEM consent page may consume
                # the first tap during its transition. Retry this reversible
                # home action once, only while still explicitly disconnected.
                retried = True
                pressed = self.click('connect', already='disconnect') or pressed
            time.sleep(.3)
        raise RuntimeError('The ordinary foreground connection did not become ready')

    def revealed_credentials(self):
        """Read the normal UI's explicitly revealed value once; return it only in memory."""
        try:
            revealed = False
            for _ in range(3):
                self.click('show', already='hide')
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline:
                    tree = self.tree()
                    if any(item.get('text') in LABELS['hide'] for item in tree.iter('node')):
                        revealed = True
                        break
                    time.sleep(.2)
                if revealed:
                    break
            if not revealed:
                raise RuntimeError('The ordinary home did not reveal the password')
            nonce = str(uuid.uuid4())
            adb(self.serial, 'shell', 'am', 'broadcast', '-n',
                f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
                '--es', 'production_request_id', nonce, '--es', 'probe_action', 'home-credentials')
            deadline = time.monotonic() + 3
            while True:
                raw = adb(self.serial, 'shell', 'run-as', PACKAGE,
                    'cat', 'files/home-credentials.json', check=False)
                value = json.loads(raw) if raw.strip() else {}
                if value.get('request_id') == nonce or time.monotonic() >= deadline:
                    break
                time.sleep(.1)
            if value.get('request_id') != nonce:
                raise RuntimeError('No revealed credentials for this one-time request')
            return value['device_code'], value['password']
        finally:
            for name in ('home-credentials.json', 'home-credentials.json.bak', 'home-credentials.json.new'):
                adb(self.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', 'files/' + name, check=False)
            adb(self.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n',
                f'{PACKAGE}/com.pixels.agent.MainActivity')
            time.sleep(.5)


def normal_status(serial):
    """Read fixed debug status without observing a screen; never use to probe a force-stopped app."""
    nonce = str(uuid.uuid4())
    adb(serial, 'shell', 'am', 'broadcast', '-n',
        f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
        '--es', 'production_request_id', nonce, '--es', 'probe_action', 'normal-status')
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        value = json.loads(adb(serial, 'shell', 'run-as', PACKAGE, 'cat', 'files/normal-status.json'))
        if value.get('request_id') == nonce:
            return value
        time.sleep(.1)
    raise RuntimeError('No current normal service status for this request')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['start-pair', 'start', 'pause', 'resume', 'disconnect'])
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', type=uuid.UUID)
    options = parser.parse_args()
    home = Home(options.serial)
    if options.action in ('start', 'start-pair'):
        # The test setup must have finished restoring the phone before normal UI setup.
        result = subprocess.run(['adb', '-s', options.serial, 'shell', 'run-as', PACKAGE,
            'test', '-d', 'no_backup/production-probe-backup'], capture_output=True)
        if result.returncode == 0:
            raise RuntimeError('Finish the active local production probe before normal home setup')
        pressed = home.start()
        if options.action == 'start':
            print('Ordinary home connection button pressed')
            return
        if options.public_run_id is None:
            parser.error('start-pair requires --public-run-id')
        root = ROOT / '.build/android/public' / str(options.public_run_id)
        session_path = root / 'session.json'
        session = json.loads(session_path.read_text(encoding='utf-8'))
        actual_hash = installed_apk_sha256(options.serial)
        if actual_hash != session['apk_sha256']:
            if session.get('normal_pairing') or any(root.glob('campaign-*')) or any(root.glob('soak-*')):
                raise RuntimeError('The phone has another acceptance APK; verify the upgrade before reusing evidence')
            session.setdefault('pre_pair_artifacts', []).append(session['apk_sha256'])
            session['apk_sha256'] = actual_hash
            session_path.write_text(json.dumps(session, indent=2), encoding='utf-8')
        try:
            home.click('show')
            time.sleep(.3)
            settings = tomllib.loads(adb(options.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml'))
            if settings.get('control_url') != 'wss://pab.rgaa.vip/control' or settings.get('trusted_ca_pem'):
                raise RuntimeError('Normal public setup must use the default server without a test certificate')
            nonce = str(uuid.uuid4())
            adb(options.serial, 'shell', 'am', 'broadcast', '-n', f'{PACKAGE}/com.pixels.agent.fixture.ProductionProbeReceiver',
                '--es', 'production_request_id', nonce, '--es', 'probe_action', 'home-credentials')
            value = json.loads(adb(options.serial, 'shell', 'run-as', PACKAGE, 'cat', 'files/home-credentials.json'))
            if value.get('request_id') != nonce:
                raise RuntimeError('No revealed credentials were produced for this setup request')
            code, password = value.pop('device_code'), value.pop('password')
            for name in ('home-credentials.json', 'home-credentials.json.bak', 'home-credentials.json.new'):
                adb(options.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', 'files/' + name, check=False)
            home.click('hide')
            paired = subprocess.run([sys.executable, str(ROOT / 'scripts/android_public.py'), 'pair',
                '--run-id', str(options.public_run_id), '--serial', options.serial, '--device-code', code],
                input=password, text=True, encoding='utf-8', capture_output=True, timeout=180)
            del password
            if paired.returncode:
                (root / 'pair-setup.log').write_text(paired.stderr, encoding='utf-8')
                raise RuntimeError('Normal public pairing did not pass; inspect private pair-setup.log')
        finally:
            for name in ('home-credentials.json', 'home-credentials.json.bak', 'home-credentials.json.new'):
                adb(options.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', 'files/' + name, check=False)
            # Recreating our Activity also clears a revealed field if setup failed mid-step.
            adb(options.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n',
                f'{PACKAGE}/com.pixels.agent.MainActivity')
        session = json.loads(session_path.read_text(encoding='utf-8'))
        session['normal_home_start'] = True
        session['home_button_pressed_in_this_run'] = pressed
        session_path.write_text(json.dumps(session, indent=2), encoding='utf-8')
        print('Normal phone home connection and public device-code/password pairing passed')
    else:
        adb(options.serial, 'shell', 'am', 'start', '-f', '0x14000000', '-n',
            f'{PACKAGE}/com.pixels.agent.MainActivity')
        if options.action in ('pause', 'resume'):
            home.set_paused(options.action == 'pause')
        else:
            home.click(options.action)
        print(f'Ordinary home action pressed: {options.action}')


if __name__ == '__main__':
    main()
