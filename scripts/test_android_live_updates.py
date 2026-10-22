#!/usr/bin/env python3
"""Verify password refresh and device rename without closing an authenticated session."""
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
from android_public import PublicMcp
from service_config import write_config
from test_android_remote import ROOT, PACKAGE, account, adb, environment, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', choices=['e2b3b128'], required=True)
    parser.add_argument('--password-only', action='store_true')
    args = parser.parse_args()
    run = ROOT / '.build/android/live-updates' / str(uuid.uuid4())
    run.mkdir(parents=True)
    config = ROOT / '.build/android/live-updates/controller/mcp.toml'
    executable = ROOT / 'target/debug/pab-mcp.exe'
    if not config.exists():
        config.parent.mkdir(parents=True, exist_ok=True)
        write_config(config, {'control_url': 'wss://pab.rgaa.vip/control',
            'relay_urls': ['https://pab-relay.rgaa.vip'], 'operator': {'data_dir': config.parent/'operator'}})
        try:
            account(config, 'register', uuid.uuid4().hex+uuid.uuid4().hex,
                username='android-live-'+uuid.uuid4().hex[:16], executable=executable)
        except Exception:
            config.unlink(missing_ok=True)
            raise
    home = Home(args.serial)
    for folder in ('values', 'values-zh-rCN'):
        for node in ET.parse(ROOT / 'apps/android/app/src/main/res' / folder / 'product.xml').getroot():
            key = node.attrib['name']
            LABELS[key] = (*LABELS.get(key, ()), node.text)
    LABELS['tab_settings'] = LABELS['product_tab_settings']
    result = {'status': 'failed', 'checks': [], 'apk_sha256': installed_apk_sha256(args.serial)}
    client = None
    code = password = old_password = None
    renamed = False
    original_name = tomllib.loads(adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml'))['device_name']

    def passed(name):
        result['checks'].append(name)
        print(json.dumps({'check': name, 'passed': True}), flush=True)

    def status_unchanged():
        current = normal_status(args.serial)
        result.setdefault('connection_states', []).append({key: current[key] for key in ('pid','generation','phase','remote_ready')})
        assert current['remote_ready'] and current['remote_control_allowed']
        assert current['pid'] == before['pid'] and current['generation'] == before['generation'], json.dumps({'before':{key:before[key] for key in ('pid','generation')}, 'after':{key:current[key] for key in ('pid','generation')}})

    def query_existing_connection():
        value, raw = client.tool('pab_list_windows', {'device_code': code}, allow_error=True)
        deadline = time.monotonic() + 30
        while value.get('result', {}).get('state') == 'running':
            assert time.monotonic() < deadline
            value, raw = client.tool('pab_get_operation', {'device_code': code,
                'operation_id': value['operation_ref']['operation_id'], 'wait_ms': 1000}, allow_error=True)
        assert not raw.get('isError') and value['result']['state'] == 'completed'

    def pair(value, success):
        process = subprocess.run([str(executable), '--config', str(config), 'device', 'connect', code,
            '--password-stdin'], input=value, text=True, encoding='utf-8', capture_output=True,
            env=environment(True), timeout=85)
        if success:
            if process.returncode != 0:
                raise RuntimeError('Test controller pairing failed: '+process.stderr.replace(value, '[redacted]')[-600:])
            assert json.loads(process.stdout)['connected']
        else:
            assert process.returncode != 0 and 'the device rejected authentication' in process.stderr

    def names():
        return {node.get('text') for node in home.tree().iter('node')}

    def rename(value, expect_rate_limit=False):
        home.click('tab_settings')
        assert not names().intersection(LABELS['product_access_password'])
        home.click('product_device_name')
        nodes = list(home.tree().iter('node'))
        target = next(node for node in nodes if node.get('resource-id') == 'device_name'
                      or node.get('class') == 'android.widget.EditText')
        left, top, right, bottom = map(int, re.findall(r'\d+', target.get('bounds')))
        adb(args.serial, 'shell', 'input', 'tap', str((left+right)//2), str((top+bottom)//2))
        adb(args.serial, 'shell', 'input', 'keyevent', '123')
        adb(args.serial, 'shell', 'input', 'keyevent', *(['67'] * 80))
        adb(args.serial, 'shell', 'input', 'text', value.replace(' ', '%s'))
        home.click('product_save')
        deadline = time.monotonic()+18
        while time.monotonic() < deadline:
            if expect_rate_limit:
                if names().intersection(LABELS['product_name_rate_limited']):
                    home.click('product_cancel')
                    return
            else:
                saved = tomllib.loads(adb(args.serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml'))
                if saved['device_name'] == value:
                    return
            time.sleep(.3)
        raise AssertionError('Device rename did not reach the expected result')

    try:
        adb(args.serial, 'shell', 'am', 'start', '-f', '0x10008000', '-n', PACKAGE+'/com.pixels.agent.MainActivity')
        home.click('tab_home')
        before = normal_status(args.serial)
        result['initial_connection'] = {key: before[key] for key in ('pid','generation','phase','remote_ready')}
        assert before['remote_ready'] and before['remote_control_allowed']
        code, password = home.revealed_credentials()
        pair(password, True)
        client = PublicMcp(config, code, True, run/'mcp.log', executable)
        connected, raw = client.connect()
        assert connected.get('connected') and not raw.get('isError')
        query_existing_connection()
        passed('authenticated_remote_connection_established')
        for _ in range(3):
            old_password = password
            home.click('rotate')
            deadline = time.monotonic()+15
            while time.monotonic() < deadline:
                _, password = home.revealed_credentials()
                if password != old_password:
                    break
                time.sleep(.2)
            assert password != old_password
            query_existing_connection()
            status_unchanged()
        passed('three_online_password_refreshes_preserve_authenticated_connection')
        pair(old_password, False)
        query_existing_connection()
        passed('old_password_rejected_for_new_connection_while_existing_session_works')
        pair(password, True)
        query_existing_connection()
        status_unchanged()
        passed('new_password_accepts_new_connections_without_interrupting_existing_one')
        if args.password_only:
            result['status'] = 'passed'
            return
        rename('Pixels Android live check')
        renamed = True
        query_existing_connection()
        status_unchanged()
        passed('online_device_rename_and_removed_settings_password_entry')
        time.sleep(10.2)
        rename(original_name)
        renamed = False
        query_existing_connection()
        status_unchanged()
        passed('original_device_name_restored_without_restarting_service')
        subprocess.run(['adb','-s',args.serial,'exec-out','screencap','-p'],stdout=(run/'settings.png').open('wb'),check=True)
        result['status'] = 'passed'
    finally:
        if renamed:
            time.sleep(10.2)
            try:
                adb(args.serial, 'shell', 'input', 'keyevent', '4')
                rename(original_name)
            except Exception:
                result['device_name_restore_required'] = True
        password = old_password = None
        if client:
            client.close()
        (run/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        print(str(run/'result.json'),flush=True)


if __name__ == '__main__':
    main()
