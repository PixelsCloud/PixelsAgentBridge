"""Verify an installed Release APK over its ordinary, already paired MCP connection.

Prepare the unlocked phone on Pixels' home screen with its password hidden.
ADB only reads package metadata and a temporary UI snapshot; no private app files,
fixture activities, settings changes, installs or input events are used.
"""
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import sys
import time
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / 'scripts'))
from scripts.test_android_remote import adb, installed_apk_sha256
from smoke.mcp_client import McpClient


def has_device_code(texts, code):
    return any(re.sub(r'\s', '', text or '') == code for text in texts)


def verify_phone(serial, package, code, expected_hash):
    if installed_apk_sha256(serial, package) != expected_hash:
        raise ValueError('Installed APK differs from the candidate; do not change the phone implicitly')
    metadata = adb(serial, 'shell', 'dumpsys', 'package', package)
    flags = re.findall(r'(?:pkgFlags|flags)=\[([^]]*)\]', metadata)
    if not flags or any('DEBUGGABLE' in row.split() for row in flags):
        raise ValueError('Release acceptance requires a verified non-debuggable installed package')
    path = '/data/local/tmp/pab-release-' + str(uuid.uuid4()) + '.xml'
    try:
        adb(serial, 'shell', 'uiautomator', 'dump', path)
        tree = ET.fromstring(adb(serial, 'shell', 'cat', path))
        texts = [node.get('text') for node in tree.iter('node') if node.get('package') == package]
        if not has_device_code(texts, code):
            raise ValueError('Show the selected phone/device code on the ordinary Pixels home screen')
    finally:
        adb(serial, 'shell', 'rm', '-f', path)


def mobile(client, code, command):
    request = str(uuid.uuid4())
    args = {'device_code': code, 'operation': 'execute', 'request_id': request, 'command': command}
    value = client.call('pab_mobile_control', args)
    deadline = time.monotonic() + 30
    while value.get('operation', {}).get('state') == 'running' and time.monotonic() < deadline:
        time.sleep(.1)
        value = client.call('pab_mobile_control', {'device_code': code, 'operation': 'get', 'request_id': request})
    reply = value.get('operation', {})
    if reply.get('state') != 'completed' or reply.get('request_id') != request:
        raise RuntimeError('Mobile request did not complete; query the original request: ' + request)
    return reply


def run(serial, apk, mcp, config, package, code, output):
    output = Path(output).resolve()
    if not output.is_relative_to((ROOT / 'smoke/runs').resolve()):
        raise ValueError('Android evidence must stay under smoke/runs')
    if not re.fullmatch(r'[0-9]{9}', code) or not re.fullmatch(r'[A-Za-z]\w*(?:\.[A-Za-z]\w*)+', package):
        raise ValueError('Supply an Android package and nine-digit device code')
    for path in (apk, mcp, config):
        if not Path(path).is_file():
            raise ValueError('Supply existing APK, MCP and normally paired operator configuration')
    expected_hash = hashlib.sha256(Path(apk).read_bytes()).hexdigest()
    output.mkdir(parents=True, exist_ok=True)
    report = {'status': 'fail', 'cleanup': False, 'checks': [], 'apk_sha256': expected_hash,
              'package': package, 'device_code': code, 'serial': serial, 'scope': 'release-readonly'}
    counts = {'passed': 0, 'failed': 0, 'skipped': 0}

    def passed(name):
        report['checks'].append(name)
        counts['passed'] += 1

    try:
        verify_phone(serial, package, code, expected_hash)
        passed('installed_release_hash_and_usb_device_identity')
        env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
        with McpClient(Path(mcp).resolve(), output / 'mcp.log', config=Path(config).resolve(), env=env) as client:
            try:
                connected = client.call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                deadline = time.monotonic() + 45
                while not connected.get('connected') and connected.get('state') == 'connecting' and time.monotonic() < deadline:
                    connected = client.call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                if not connected.get('connected') or connected.get('target', {}).get('execution', {}).get('os_family') != 'android':
                    raise RuntimeError('Normal authenticated Android connection was not established')
                if connected.get('connection_path') not in ('p2p', 'relay', 'direct'):
                    raise RuntimeError('Actual connection path is missing')
                report['connection_path'] = connected['connection_path']
                passed('normal_android_connection')
                caps = mobile(client, code, {'action': 'capabilities'})['capabilities']
                if not {'observe', 'query'}.issubset(caps.get('actions', [])):
                    raise RuntimeError('Required Android observation capabilities are missing')
                passed('advertised_mobile_capabilities')
                observed = mobile(client, code, {'action': 'observe'})
                screen = observed.get('observation') or {}
                if (screen.get('package_name') != package or not screen.get('width') or not screen.get('height')
                        or not has_device_code([node.get('name') for node in screen.get('elements', [])], code)):
                    raise RuntimeError('Remote home observation does not match the selected USB phone')
                passed('remote_home_matches_usb_phone')
                original = client.call('pab_mobile_control', {'device_code': code, 'operation': 'get',
                    'request_id': observed['request_id']})
                if original.get('operation') != observed:
                    raise RuntimeError('Querying the original request changed its completed result')
                passed('original_mobile_result_preserved')
                query = mobile(client, code, {'action': 'query', 'max_nodes': 128, 'max_depth': 32})
                if not (query.get('observation') or {}).get('elements'):
                    raise RuntimeError('Android UI query returned no visible home controls')
                passed('normal_ui_query')
                if installed_apk_sha256(serial, package) != expected_hash:
                    raise RuntimeError('Installed candidate changed during acceptance')
                passed('candidate_unchanged')
            finally:
                disconnected = client.call('pab_disconnect', {'device_code': code})
                if disconnected.get('error') or disconnected.get('disconnected') is not True:
                    raise RuntimeError('Owned MCP connection cleanup was not confirmed')
                report['cleanup'] = True
        report['status'] = 'pass'
    except BaseException:
        counts['failed'] += 1
        raise
    finally:
        (output / 'android-result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
        (output / 'counts.json').write_text(json.dumps(counts), encoding='utf-8')
    return 0


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('serial', 'apk', 'mcp', 'config', 'package', 'code', 'output'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    raise SystemExit(run(args.serial, args.apk, args.mcp, args.config, args.package, args.code, args.output))
