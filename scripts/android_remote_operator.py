#!/usr/bin/env python3
"""Run real public MCP postcondition loops from an isolated remote host, without ADB."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

from android_public import PublicMcp
from android_scenarios import AndroidTasks
from service_config import write_config
from test_android_remote import PACKAGE, account, environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['pair', 'check-old', 'advance', 'campaign', 'logout'])
    parser.add_argument('--root', required=True, type=Path)
    parser.add_argument('--run-id', required=True, type=uuid.UUID)
    parser.add_argument('--apk-sha256', required=True)
    parser.add_argument('--rounds', type=int, default=20)
    parser.add_argument('--relay-only', action='store_true')
    parser.add_argument('--upgrade-proof', type=Path)
    parser.add_argument('--network-label', choices=['preflight', 'wifi', 'cellular', 'direct', 'fallback'],
        default='preflight')
    args = parser.parse_args()
    assert 1 <= args.rounds <= 100
    root = args.root.resolve()
    assert root.name == str(args.run_id)
    config = root / 'mcp.toml'
    executable = root / ('pab-mcp.exe' if os.name == 'nt' else 'pab-mcp')
    metadata = root / 'session.json'
    if args.action in ('pair', 'check-old'):
        assert not metadata.exists(), 'Do not overwrite a paired operator session'
        credential = json.loads(sys.stdin.read(2048))
        assert credential['run_id'] == str(args.run_id)
        code, password = credential['device_code'], credential['password']
        assert len(code) == 9 and code.isascii() and code.isdigit() and password
        old_check = root / 'old-password-check.json'
        if not config.exists():
            write_config(config, {'control_url':'wss://pab.rgaa.vip/control',
                'relay_urls':['https://pab-relay.rgaa.vip'], 'operator':{'data_dir':root / 'operator'}})
            account(config, 'register', uuid.uuid4().hex + uuid.uuid4().hex, executable=executable,
                username='android-remote-' + args.run_id.hex[:16])
        else:
            import tomllib
            previous = json.loads(old_check.read_text(encoding='utf-8'))
            assert previous['status'] == 'passed' and previous['run_id'] == str(args.run_id)
            existing = tomllib.loads(config.read_text(encoding='utf-8'))
            assert existing['control_url'] == 'wss://pab.rgaa.vip/control'
            assert Path(existing['operator']['data_dir']).resolve() == root / 'operator'
        command = [str(executable), '--config', str(config), 'device', 'connect', code, '--password-stdin']
        if args.action == 'check-old':
            old = subprocess.run(command, input=password, text=True, encoding='utf-8',
                capture_output=True, env=environment(True), timeout=85)
            del credential, password
            assert old.returncode and 'the device rejected authentication' in old.stderr
            old_check.write_text(json.dumps({'status':'passed', 'run_id':str(args.run_id),
                'device_code':code, 'old_password_rejected':True,
                'provenance':{'apk_sha256':args.apk_sha256, 'public_server':True},
                'credential_origin':'pre_rotation_normal_home_sealed_envelope',
                'observed_at_unix':time.time()}, indent=2), encoding='utf-8')
            print('Pre-rotation password rejected by the real device', flush=True)
            return
        wrong = subprocess.run(command, input='invalid-' + uuid.uuid4().hex, text=True,
            encoding='utf-8', capture_output=True, env=environment(True), timeout=85)
        assert wrong.returncode and 'the device rejected authentication' in wrong.stderr
        paired = subprocess.run(command, input=password, text=True, encoding='utf-8',
            capture_output=True, env=environment(True), timeout=85)
        del credential, password
        assert paired.returncode == 0 and json.loads(paired.stdout)['connected'] is True, 'Normal pairing failed'
        metadata.write_text(json.dumps({'run_id':str(args.run_id), 'apk_sha256':args.apk_sha256,
            'device_code':code, 'normal_pairing':True, 'wrong_password_rejected':True,
            'created_at_unix':time.time()}, indent=2), encoding='utf-8')
        print('Remote isolated account and normal device pairing passed', flush=True)
        return
    session = json.loads(metadata.read_text(encoding='utf-8'))
    assert session['run_id'] == str(args.run_id)
    if args.action == 'advance':
        assert args.upgrade_proof is not None
        proof = json.loads(args.upgrade_proof.read_text(encoding='utf-8'))
        previous = proof['phone']
        assert previous['apk_sha256'] == session['apk_sha256']
        updates = proof['upgrades']
        assert updates and updates[-1]['apk_sha256'] == args.apk_sha256
        fields = ('certificate_sha256', 'first_install_time', 'fingerprints', 'paused', 'connection_requested')
        assert len(previous['fingerprints']) >= 5 and previous['paused'] is True
        assert all(all(row[field] == previous[field] for field in fields) for row in updates)
        assert all(right['version_code'] > left['version_code']
            for left, right in zip([previous, *updates[:-1]], updates))
        old_results = [json.loads(path.read_text(encoding='utf-8')) for path in root.glob('campaign-*/result.json')]
        old_result = next(value for value in old_results if value.get('status') == 'passed'
            and value['provenance']['apk_sha256'] == session['apk_sha256'] and value.get('original_request_id'))
        transition = {'status':'failed', 'from_apk_sha256':session['apk_sha256'],
            'to_apk_sha256':args.apk_sha256, 'device_code':session['device_code']}
        client = PublicMcp(config, session['device_code'], True, root / 'upgrade-check.log', executable)
        try:
            connected, raw = client.connect()
            assert connected['connected'] and not raw.get('isError')
            assert connected['device_ref'] == proof['device_ref']
            old, _ = client.mobile(old_result['original_request_id'], operation='get')
            assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
            blocked, _ = client.mobile(str(uuid.uuid4()), {'action':'navigate', 'destination':'home'})
            assert blocked['operation']['error'] == 'control_paused' and not blocked['operation']['action_dispatched']
            transition.update(status='passed', device_ref=connected['device_ref'],
                cached_normal_credentials_valid=True, original_request_query='completed',
                paused_input_not_dispatched=True, observed_at_unix=time.time())
        finally:
            client.close()
            (root / ('artifact-transition-' + str(uuid.uuid4()) + '.json')).write_text(
                json.dumps(transition, indent=2), encoding='utf-8')
        session.setdefault('artifact_transitions', []).append(transition)
        session['apk_sha256'] = args.apk_sha256
        metadata.write_text(json.dumps(session, indent=2), encoding='utf-8')
        print('Remote normal pairing retained across the verified cover upgrade', flush=True)
        return
    assert session['apk_sha256'] == args.apk_sha256
    if args.action == 'logout':
        account(config, 'logout', executable=executable)
        return
    campaign = root / ('campaign-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'run_id':campaign.name, 'provenance':{**session,
        'public_server':True, 'forced_relay':args.relay_only, 'network_label':args.network_label},
        'physical_usb_unplug_confirmed':False, 'network_environment_verified':False,
        'requested_rounds':args.rounds, 'passed_rounds':0, 'started_at_unix':time.time()}
    client = None
    try:
        client = PublicMcp(config, session['device_code'], args.relay_only, campaign / 'mcp.log', executable)
        connected, raw = client.connect()
        assert not raw.get('isError') and connected['connected'] is True
        assert connected['target']['execution']['os_family'] == 'android'
        result['device_ref'] = connected['device_ref']
        tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
        tasks.call({'action':'capabilities'})
        if args.network_label == 'direct':
            assert not args.relay_only
            deadline = time.monotonic() + 45
            while True:
                observed, _ = client.mobile(str(uuid.uuid4()), {'action':'observe'})
                assert observed['operation']['state'] == 'completed'
                if observed.get('connection_path') == 'direct':
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError('This network did not establish a verified Direct path')
                time.sleep(.5)
        timings = []
        rounds = []
        for number in range(args.rounds):
            started = time.monotonic()
            previous_paths = tasks.paths.copy()
            tasks.fixture()
            before = tasks.count()
            text = f'跨网验收-{args.run_id.hex[:8]}-{number}'
            tasks.act('fixture_input', 'set_text', text=text)
            tasks.act('fixture_save')
            tasks.wait(text)
            assert tasks.count() == before + 1, 'Input postcondition or duplicate count failed'
            paths = dict(tasks.paths - previous_paths)
            if args.network_label == 'direct':
                assert set(paths) == {'direct'}, 'A Direct round fell back; do not count it as Direct'
            if args.relay_only:
                assert set(paths) == {'relay'}, 'A forced Relay round used another path'
            result['passed_rounds'] += 1
            timings.append(round((time.monotonic() - started) * 1000))
            rounds.append({'index':number, 'postcondition':True, 'connection_paths':paths})
            (campaign / 'progress.json').write_text(json.dumps({'passed_rounds':result['passed_rounds'],
                'requested_rounds':args.rounds}), encoding='utf-8')
            print(f'Remote closed loop {number + 1}/{args.rounds} passed', flush=True)
        original = tasks.last_input_request
        client.close()
        client = PublicMcp(config, session['device_code'], args.relay_only, campaign / 'reconnect.log', executable)
        connected, raw = client.connect()
        assert not raw.get('isError') and connected['connected'] is True
        assert connected['device_ref'] == result['device_ref']
        old, _ = client.mobile(original, operation='get')
        assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
        result.update(status='passed', original_request_query='completed', round_elapsed_ms=timings,
            original_request_id=original, rounds=rounds, observed_connection_paths=dict(tasks.paths),
            calls=tasks.calls)
    except Exception as error:
        result['failure'] = str(error)
        raise
    finally:
        if client:
            client.close()
        result['finished_at_unix'] = time.time()
        (campaign / 'result.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'],
            'passed_rounds':result['passed_rounds']}), flush=True)


if __name__ == '__main__':
    main()
