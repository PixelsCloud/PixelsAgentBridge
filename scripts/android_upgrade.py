#!/usr/bin/env python3
"""Verify cover upgrades against a real public device and an existing MCP input fact."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib
import uuid

from android_public import PublicMcp
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def phone_state(serial):
    apk = ROOT / 'apps/android/app/build/outputs/apk/debug/app-debug.apk'
    digest = hashlib.sha256(apk.read_bytes()).hexdigest()
    assert installed_apk_sha256(serial) == digest
    package = adb(serial, 'shell', 'dumpsys', 'package', PACKAGE)
    settings = tomllib.loads(adb(serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/settings.toml'))
    assert settings['control_url'] == 'wss://pab.rgaa.vip/control' and not settings.get('trusted_ca_pem')
    scope = hashlib.sha256(settings['control_url'].encode()).hexdigest()
    files = ['no_backup/settings.toml', 'no_backup/control-intent.toml'] + [
        f'no_backup/credentials/{scope}-{purpose}.bin' for purpose in ('seed', 'access', 'registration')]
    fingerprints = {path:adb(serial, 'shell', 'run-as', PACKAGE, 'sha256sum', path).split()[0] for path in files}
    intent = tomllib.loads(adb(serial, 'shell', 'run-as', PACKAGE, 'cat', 'no_backup/control-intent.toml'))
    sdk = Path(os.environ.get('ANDROID_HOME', 'D:/android/sdk'))
    signer = sorted(sdk.glob('build-tools/*/apksigner.bat' if os.name == 'nt' else 'build-tools/*/apksigner'))[-1]
    env = dict(os.environ, JAVA_TOOL_OPTIONS='-XX:ActiveProcessorCount=2')
    java = Path('C:/Program Files/Android/Android Studio/jbr')
    if java.exists():
        env['JAVA_HOME'] = str(java)
    result = subprocess.run([str(signer), 'verify', '--print-certs', str(apk)],
        capture_output=True, text=True, encoding='utf-8', env=env, check=True)
    certificate = re.search(r'Signer #1 certificate SHA-256 digest: ([a-fA-F0-9]{64})', result.stdout)[1].lower()
    return {'apk_sha256':digest, 'version_code':int(re.search(r'versionCode=(\d+)', package)[1]),
        'certificate_sha256':certificate, 'first_install_time':re.search(r'firstInstallTime=([^\r\n]+)', package)[1],
        'fingerprints':fingerprints, 'paused':intent['paused'], 'connection_requested':intent['connection_requested']}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['baseline', 'verify'])
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--campaign', type=Path, help='Successful public campaign directory for the baseline')
    parser.add_argument('--baseline-file', default='upgrade-baseline.json',
        help='Independent cohort filename after an intentional credential change')
    options = parser.parse_args()
    root = ROOT / '.build/android/public' / str(options.public_run_id)
    session_path = root / 'session.json'
    session = json.loads(session_path.read_text(encoding='utf-8'))
    if not re.fullmatch(r'upgrade-baseline(?:-[a-z0-9-]{1,64})?\.json', options.baseline_file):
        parser.error('The baseline filename must remain a named upgrade-baseline JSON file')
    baseline_path = root / options.baseline_file
    state = phone_state(options.serial)
    assert state['paused'] is True, 'Pause through the normal home control before recording an upgrade baseline'
    if options.action == 'baseline':
        assert not baseline_path.exists(), 'Do not overwrite an existing upgrade baseline'
        if options.campaign is None:
            parser.error('Baseline requires the successful public campaign directory')
        campaign = options.campaign.resolve()
        assert campaign.is_relative_to(root.resolve())
        result = json.loads((campaign / 'result.json').read_text(encoding='utf-8'))
        assert result['status'] == 'passed' and result['provenance']['apk_sha256'] == state['apk_sha256']
        trace = [json.loads(line) for line in (campaign / 'operation-trace.jsonl').read_text(encoding='utf-8').splitlines()]
        original = next(value['request_id'] for value in trace if value['action'] == 'set_text'
                        and value['state'] == 'completed' and value['action_dispatched'])
        baseline = {'phone':state, 'original_request':original, 'device_code':session['device_code'], 'upgrades':[]}
    else:
        baseline = json.loads(baseline_path.read_text(encoding='utf-8'))
        original = baseline['original_request']
        previous = baseline['upgrades'][-1] if baseline['upgrades'] else baseline['phone']
        assert state['version_code'] > previous['version_code'], 'A cover upgrade must increase versionCode'
        for field in ('certificate_sha256', 'first_install_time', 'fingerprints', 'paused', 'connection_requested'):
            assert state[field] == baseline['phone'][field], f'Upgrade changed retained state: {field}'
    executable = root / ('pab-mcp.exe' if os.name == 'nt' else 'pab-mcp')
    client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
        root / f'upgrade-v{state["version_code"]}.log', executable)
    try:
        connected, raw = client.connect()
        assert not raw.get('isError') and connected.get('connected') is True
        assert connected['target']['execution']['os_family'] == 'android'
        identity = connected['device_ref']
        reply, _ = client.mobile(original, operation='get')
        assert reply['operation']['state'] == 'completed' and reply['operation']['action_dispatched']
        blocked, _ = client.mobile(str(uuid.uuid4()), {'action':'navigate', 'destination':'home'})
        assert blocked['operation']['error'] == 'control_paused' and not blocked['operation']['action_dispatched']
    finally:
        client.close()
    if options.action == 'baseline':
        baseline['device_ref'] = identity
    else:
        assert identity == baseline['device_ref'] and session['device_code'] == baseline['device_code']
        baseline['upgrades'].append({**state, 'original_request_query':'completed', 'paused_input_not_dispatched':True})
        session['apk_sha256'] = state['apk_sha256']
        session_path.write_text(json.dumps(session, indent=2), encoding='utf-8')
    baseline_path.write_text(json.dumps(baseline, indent=2), encoding='utf-8')
    print(json.dumps({'status':'passed', 'action':options.action, 'version_code':state['version_code'],
        'verified_cover_upgrades':len(baseline['upgrades']), 'original_request_query':'completed',
        'paused_input_not_dispatched':True, 'apk_sha256':state['apk_sha256']}))


if __name__ == '__main__':
    main()
