#!/usr/bin/env python3
"""Verify normal Android connection stops after real control authentication rejection.

Limited to a disposable emulator. Its settings and control intent are copied before
the isolated TLS fixture is selected and restored byte-for-byte in finally.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import uuid

from android_home import Home, normal_status
from android_accessibility import set_enabled
from service_config import write_config
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    args = parser.parse_args()
    assert args.serial.startswith('emulator-'), 'Fault injection is limited to the disposable emulator'
    run_id = str(uuid.uuid4())
    directory = ROOT / '.build/android/terminal-rejection' / run_id
    directory.mkdir(parents=True)
    digest = installed_apk_sha256(args.serial)
    result = {'status':'failed', 'provenance':{'serial':args.serial, 'apk_sha256':digest},
        'scope':'normal_connection_control_authentication_rejection', 'samples':[]}
    executable = ROOT / 'target/debug/examples' / ('control-fixture.exe' if os.name == 'nt' else 'control-fixture')
    lines = queue.Queue()
    events = []
    log = (directory / 'fixture-stderr.log').open('w', encoding='utf-8')
    server = subprocess.Popen([str(executable), '--reject-authentication'], stdout=subprocess.PIPE,
        stderr=log, text=True, encoding='utf-8')

    def consume():
        for line in server.stdout:
            value = json.loads(line)
            lines.put(value)
            if value.get('event'):
                events.append(value['event'])

    reader = threading.Thread(target=consume, daemon=True)
    reader.start()
    originals = ('no_backup/settings.toml', 'no_backup/control-intent.toml')
    backups = {}
    mapped_port = None
    staged = '/data/local/tmp/pab-terminal-' + run_id + '.toml'
    try:
        ready = lines.get(timeout=10)
        assert ready['ready'] is True
        for index, name in enumerate(originals):
            backup = f'files/terminal-{run_id}-{index}.toml'
            fingerprint = adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum', name).split()[0]
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', name, backup)
            backups[name] = (backup, fingerprint)
        # A new foreground launch clears the emulator's old process after disconnect.
        adb(args.serial, 'shell', 'am', 'force-stop', PACKAGE)
        write_config(directory / 'settings.toml', {'schema_version':1,
            'control_url':f'wss://localhost:{ready["port"]}/control', 'device_name':'Terminal rejection fixture',
            'relay_urls':[], 'trusted_ca_pem':ready['ca_pem']})
        adb(args.serial, 'push', str(directory / 'settings.toml'), staged)
        adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', staged, 'no_backup/settings.toml')
        mapped_port = ready['port']
        adb(args.serial, 'reverse', f'tcp:{mapped_port}', f'tcp:{mapped_port}')
        adb(args.serial, 'shell', 'am', 'start', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
        set_enabled(args.serial, True)
        time.sleep(.8)
        Home(args.serial).click('connect')
        deadline = time.monotonic() + 40
        while True:
            status = normal_status(args.serial)
            if status['phase'] == 'FAILED':
                break
            assert time.monotonic() < deadline, 'Normal connection did not stop after rejection'
            time.sleep(.2)
        stopped_at = time.monotonic()
        while time.monotonic() - stopped_at < 15:
            status = normal_status(args.serial)
            assert status['phase'] == 'FAILED' and status['remote_ready'] is False
            result['samples'].append({'elapsed_seconds':round(time.monotonic() - stopped_at, 2),
                'phase':status['phase'], 'remote_ready':status['remote_ready'], 'pid':status['pid']})
            time.sleep(1)
        assert events.count('registered') == 1 and events.count('authentication_rejected') == 1
        assert events.count('authenticated') == 0
        result.update(status='passed', authentication_rejections=1, repeated_authentication_attempts=0,
            observation_seconds=round(time.monotonic() - stopped_at, 2))
    finally:
        adb(args.serial, 'shell', 'am', 'force-stop', PACKAGE, check=False)
        for name, (backup, fingerprint) in backups.items():
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'cp', backup, name)
            assert adb(args.serial, 'shell', 'run-as', PACKAGE, 'sha256sum', name).split()[0] == fingerprint
            adb(args.serial, 'shell', 'run-as', PACKAGE, 'rm', '-f', backup)
        result['original_settings_and_intent_restored'] = len(backups) == len(originals)
        adb(args.serial, 'shell', 'rm', '-f', staged, check=False)
        if mapped_port is not None:
            adb(args.serial, 'reverse', '--remove', f'tcp:{mapped_port}', check=False)
        server.terminate()
        server.wait(timeout=5)
        reader.join(timeout=2)
        log.close()
        adb(args.serial, 'shell', 'am', 'start', '-n', f'{PACKAGE}/com.pixels.agent.MainActivity')
        (directory / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'directory':str(directory), 'status':result['status']}), flush=True)


if __name__ == '__main__':
    main()
