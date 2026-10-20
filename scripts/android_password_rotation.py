#!/usr/bin/env python3
"""Verify the normal home password reset, old/new authentication and retained request facts."""
import argparse
import json
import subprocess
import time
import uuid

from android_home import Home, normal_status
from android_public import PublicMcp
from android_upgrade import phone_state
from test_android_remote import ROOT, environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--original-request', required=True, type=uuid.UUID)
    args = parser.parse_args()
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    campaign = root / ('password-rotation-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True}, 'checks':{}}
    home = Home(args.serial)
    before = phone_state(args.serial)
    assert before['apk_sha256'] == session['apk_sha256']
    executable = root / 'pab-mcp.exe'
    command = [str(executable), '--config', str(root / 'mcp.toml'), 'device', 'connect',
        session['device_code'], '--password-stdin']
    old_password = new_password = None
    client = None
    try:
        home.start()
        code, old_password = home.revealed_credentials()
        assert code == session['device_code']
        home.click('disconnect')
        deadline = time.monotonic() + 15
        while normal_status(args.serial)['phase'] != 'DISCONNECTED':
            if time.monotonic() >= deadline:
                raise RuntimeError('Disconnect did not finish before password rotation')
            time.sleep(.2)
        home.click('rotate')
        deadline = time.monotonic() + 15
        while True:
            after = phone_state(args.serial)
            changed = [name for name in before['fingerprints']
                if before['fingerprints'][name] != after['fingerprints'][name]]
            if any(name.endswith('-access.bin') for name in changed):
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('Normal password reset did not commit a new credential')
            time.sleep(.2)
        assert after['paused'] is True
        assert all(name.endswith('-access.bin') or name.endswith('control-intent.toml') for name in changed)
        assert home.start() is True
        code, new_password = home.revealed_credentials()
        assert code == session['device_code'] and new_password != old_password
        old = subprocess.run(command, input=old_password, text=True, encoding='utf-8',
            capture_output=True, env=environment(True), timeout=85)
        assert old.returncode and 'the device rejected authentication' in old.stderr
        result['checks']['old_password_rejected'] = True
        new = subprocess.run(command, input=new_password, text=True, encoding='utf-8',
            capture_output=True, env=environment(True), timeout=85)
        assert new.returncode == 0 and json.loads(new.stdout)['connected'] is True
        result['checks']['new_password_normal_pairing'] = True
        old_password = new_password = None
        client = PublicMcp(root / 'mcp.toml', code, True, campaign / 'mcp.log', executable)
        connected, raw = client.connect()
        assert connected['connected'] is True and not raw.get('isError')
        baseline = json.loads((root / 'upgrade-baseline.json').read_text(encoding='utf-8'))
        assert connected['device_ref'] == baseline['device_ref']
        old, _ = client.mobile(str(args.original_request), operation='get')
        assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
        blocked, _ = client.mobile(str(uuid.uuid4()), {'action':'navigate', 'destination':'home'})
        assert blocked['operation']['error'] == 'control_paused' and not blocked['operation']['action_dispatched']
        result['checks'].update(identity_and_seed_retained=True, old_request_query='completed',
            reset_keeps_control_paused=True, normal_home_reset=True)
        result['original_request_id'] = str(args.original_request)
        result['status'] = 'passed'
    finally:
        old_password = new_password = None
        if client:
            client.close()
        (campaign / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'], 'checks':result['checks']}))


if __name__ == '__main__':
    main()
