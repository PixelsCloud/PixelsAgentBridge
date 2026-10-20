#!/usr/bin/env python3
"""Measure real Wi-Fi/cellular recovery on the public connection while USB stays attached.

ADB changes network conditions and reads transport metadata only. Every input and
postcondition travels over the ordinary authenticated MCP path, without ADB reverse.
This campaign never claims physical USB unplug acceptance.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import time
import uuid

from android_public import PublicMcp
from android_scenarios import AndroidTasks
from test_android_remote import ROOT, PACKAGE, adb, installed_apk_sha256


def active_network(serial):
    text = adb(serial, 'shell', 'dumpsys', 'connectivity')
    selected = re.search(r'Active default network: (\d+|none)', text)
    if selected is None:
        raise RuntimeError('The active physical network could not be identified')
    if selected[1] == 'none':
        return None
    for line in text.splitlines():
        if 'NetworkAgentInfo' not in line or f'network{{{selected[1]}}}' not in line:
            continue
        transport = re.search(r'Transports: (.*?)(?: Capabilities:| LinkUpBandwidth| LinkDnBandwidth|\])', line)
        if transport:
            return {'network_id':selected[1], 'transports':transport[1].split('|')}
    raise RuntimeError('The selected default network had no transport metadata')


def wait_transport(serial, expected, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = active_network(serial)
        if expected is None and value is None:
            return value
        if value and expected in value['transports'] and 'VPN' not in value['transports']:
            return value
        time.sleep(.4)
    raise RuntimeError('The requested physical network did not become the default')


def mobile_data_state(serial):
    # Multi-SIM devices can retain a stale legacy mobile_data flag. Do not
    # restore the selected SIM from that unrelated value.
    values = {key:value for line in adb(serial, 'shell', 'settings', 'list', 'global').splitlines()
        if '=' in line for key, value in [line.split('=', 1)]
        if re.fullmatch(r'mobile_data\d*', key)}
    indexed = {key:value for key, value in values.items() if key != 'mobile_data'}
    flags = set(indexed.values()) if indexed else {values.get('mobile_data')}
    if len(flags) != 1 or not flags <= {'0', '1'}:
        raise RuntimeError('Per-SIM data flags disagree; cannot safely infer the restoration state')
    return {'enabled':flags == {'1'}, 'per_sim_flags':indexed, 'legacy_flag':values.get('mobile_data')}


def connect_live(client, deadline):
    """Only offline presence can be polled; credentials errors stop immediately."""
    while True:
        connected, raw = client.tool('pab_connect',
            {'device_code':client.code, 'wait_ms':1000}, allow_error=True)
        if raw.get('isError'):
            error = str(connected.get('error'))
            if not ('NotFound' in error and 'the device is offline' in error):
                raise RuntimeError('Fresh control connection was rejected: ' + error)
        elif connected.get('connected'):
            value, raw = client.mobile(str(uuid.uuid4()), {'action':'observe'})
            assert not raw.get('isError') and value['operation']['state'] == 'completed'
            return value
        if time.monotonic() >= deadline:
            raise RuntimeError('Fresh public MCP did not recover after the physical network returned')
        time.sleep(.2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--public-run-id', required=True, type=uuid.UUID)
    parser.add_argument('--cycles', type=int, default=30)
    args = parser.parse_args()
    assert 2 <= args.cycles <= 100
    root = ROOT / '.build/android/public' / str(args.public_run_id)
    session = json.loads((root / 'session.json').read_text(encoding='utf-8'))
    assert installed_apk_sha256(args.serial) == session['apk_sha256']
    assert not adb(args.serial, 'reverse', '--list').strip(), 'Remove test reverse mappings first'
    assert adb(args.serial, 'shell', 'getprop', 'sys.usb.config').strip().find('rndis') == -1
    assert active_network(args.serial)['transports'] == ['WIFI'], 'Begin with normal Wi-Fi, without a VPN'
    wifi = adb(args.serial, 'shell', 'settings', 'get', 'global', 'wifi_on').strip()
    data = mobile_data_state(args.serial)
    assert wifi in ('0', '1')
    campaign = root / ('network-cycles-' + str(uuid.uuid4()))
    campaign.mkdir()
    result = {'status':'failed', 'provenance':{'apk_sha256':session['apk_sha256'],
        'serial':args.serial, 'public_server':True, 'adb_reverse':False, 'forced_relay':True},
        'physical_usb_unplug_confirmed':False, 'original_mobile_data_state':data, 'cycles':[]}
    client = PublicMcp(root / 'mcp.toml', session['device_code'], True, campaign / 'mcp.log', root / 'pab-mcp.exe')
    try:
        connect_live(client, time.monotonic() + 55)
        tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
        tasks.fixture()
        for index in range(args.cycles):
            target = 'CELLULAR' if index % 2 == 0 else 'WIFI'
            outage = index % 5 == 0
            before = tasks.count()
            original = tasks.last_input_request
            if outage:
                adb(args.serial, 'shell', 'svc', 'wifi', 'disable')
                adb(args.serial, 'shell', 'svc', 'data', 'disable')
                wait_transport(args.serial, None)
                time.sleep(3)
            restored_at = time.monotonic()
            adb(args.serial, 'shell', 'svc', 'data', 'enable')
            adb(args.serial, 'shell', 'svc', 'wifi', 'enable' if target == 'WIFI' else 'disable')
            network = wait_transport(args.serial, target)
            network_at = time.monotonic()
            deadline = network_at + 55
            # A fresh operator must resolve live control-plane presence as well
            # as recover the data path; a cached peer alone does not prove both.
            client.close()
            client = PublicMcp(root / 'mcp.toml', session['device_code'], True,
                campaign / f'reconnect-{index}.log', root / 'pab-mcp.exe')
            value = connect_live(client, deadline)
            recovered_at = time.monotonic()
            tasks = AndroidTasks(client, PACKAGE, campaign / 'operation-trace.jsonl')
            assert tasks.count() == before, 'A prior input was replayed during recovery'
            if original:
                old, _ = client.mobile(original, operation='get')
                assert old['operation']['state'] == 'completed' and old['operation']['action_dispatched']
            text = f'真实切网-{index}'
            tasks.act('fixture_input', 'set_text', text=text)
            tasks.act('fixture_save')
            tasks.wait(text)
            assert tasks.count() == before + 1
            result['cycles'].append({'index':index, 'target':target, 'full_outage':outage,
                'actual_network':network, 'network_available_ms':round((network_at - restored_at) * 1000),
                'reconnect_ms':round((recovered_at - network_at) * 1000),
                'connection_path':value.get('connection_path'), 'postcondition':True,
                'prior_input_not_replayed':True, 'fresh_control_client':True})
            (campaign / 'progress.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
            print(f'Physical network cycle {index + 1}/{args.cycles} passed: {target}', flush=True)
        values = sorted(row['reconnect_ms'] for row in result['cycles'])
        result['reconnect_p95_ms'] = values[(len(values) * 95 + 99) // 100 - 1]
        assert result['reconnect_p95_ms'] <= 30000
        result['status'] = 'passed'
    finally:
        client.close()
        adb(args.serial, 'shell', 'svc', 'data', 'enable' if data['enabled'] else 'disable')
        adb(args.serial, 'shell', 'svc', 'wifi', 'enable' if wifi == '1' else 'disable')
        restored = mobile_data_state(args.serial)
        result['restored_mobile_data_state'] = restored
        assert restored['enabled'] == data['enabled'] and restored['per_sim_flags'] == data['per_sim_flags']
        (campaign / 'result.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
        print(json.dumps({'campaign':str(campaign), 'status':result['status'],
            'completed_cycles':len(result['cycles'])}), flush=True)


if __name__ == '__main__':
    main()
