"""Two-host native batch check through normal Pixels pairing; no app install.

Build the Bridge libtest binary on each target, then provide its path and hash
in a private TOML lab file. Public offers travel only through Pixels file
transfer. This bounded check does not replace GUI, fault, performance or soak
acceptance. Run with --lab, --mcp, optional --config and --output.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import re
import time
import tomllib
import uuid

if __package__:
    from .mcp_client import McpClient
else:
    from mcp_client import McpClient

TEST = 'runtime::file_batch_network_fixture::two_host_native_batch_fixture'


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def remote_base(device, run):
    style = PureWindowsPath if device['os'] == 'windows' else PurePosixPath
    parent = style(device['parent'])
    if (device['os'] not in ('windows', 'linux', 'macos') or not parent.is_absolute()
            or len(parent.parts) < 2 or '..' in parent.parts):
        raise ValueError('Use an absolute dedicated test parent, never a filesystem root')
    return parent / f'network-{uuid.UUID(run)}'


def validate_lab(lab):
    settings = lab['file_transfer']
    if settings.get('isolated_network') is not True:
        raise ValueError('This pinned synthetic-peer fixture requires an explicit isolated network')
    if settings['mode'] not in ('auto', 'relay'):
        raise ValueError('Select auto or strict relay mode')
    devices = settings['device']
    if len(devices) != 2 or {device['role'] for device in devices} != {'client', 'server'}:
        raise ValueError('Exactly one client and one server are required')
    if len({device['code'] for device in devices}) != 2:
        raise ValueError('Use two distinct test devices')
    for device in devices:
        if not re.fullmatch(r'[0-9]{9}', device['code']):
            raise ValueError('Invalid device code')
        remote_base(device, str(uuid.uuid4()))
        if not re.fullmatch(r'[0-9a-f]{64}', device['sha256']):
            raise ValueError('Pin each target binary SHA-256')
        if sha256(device['binary']) != device['sha256']:
            raise ValueError('Local binary differs from the lab SHA-256')
    urls = settings['relay_urls']
    from urllib.parse import urlsplit
    if not 1 <= len(urls) <= 8 or any(urlsplit(url).scheme != 'https' or not urlsplit(url).hostname
            or urlsplit(url).username or urlsplit(url).password for url in urls):
        raise ValueError('Use explicit HTTPS Relay URLs without credentials')
    return settings


def execute(client, settings, output):
    run = str(uuid.uuid4())
    report = {'run': run, 'status': 'running', 'scope': 'two-host native batch basic',
              'full_acceptance': False, 'mode': settings['mode'], 'calls': [], 'hosts': {}, 'cleanup': []}
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    if (output / 'result.json').exists():
        raise ValueError('Choose a fresh report directory')

    def save():
        (output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    def call(name, arguments, *, observe=True, timeout=60, allow_error=False):
        entry = {'tool': name, 'arguments': arguments}
        report['calls'].append(entry)
        save()  # Keep accepted IDs even if the next observation fails.
        value = client.call(name, arguments, allow_error=True)
        entry['accepted'] = value
        save()
        if observe:
            value = client.observe(value, timeout=timeout)
        entry['observed'] = value
        save()
        if not allow_error and (value.get('error') or value.get('result', {}).get('state') in ('failed', 'cancelled', 'unconfirmed')):
            raise RuntimeError(f'{name} did not produce a confirmed result')
        return value

    def transfer(name, code, source, destination, expected):
        value = call(name, {'device_code': code, 'source': str(source), 'destination': str(destination),
                           'overwrite': False, 'wait_ms': 1}, timeout=300)
        operation = value.get('operation', {})
        if operation.get('state') != 'completed' or operation.get('sha256') != expected:
            raise RuntimeError('Pixels transfer did not confirm the pinned bytes')

    def task_done(host):
        value = call('pab_get_task', {'device_code': host['code'], 'task_id': host['task'], 'wait_ms': 1}, observe=False)
        return value.get('snapshot', {}).get('state') in ('succeeded', 'failed', 'cancelled')

    def stdout(host):
        return call('pab_read_output', {'device_code': host['code'], 'task_id': host['task'],
                    'stream': 'stdout', 'offset': 0, 'max_bytes': 16384}, observe=False)['text']

    save()
    try:
        # Provision both before either starts waiting for the exchanged offer.
        for device in settings['device']:
            code, role = device['code'], device['role']
            connected = call('pab_connect', {'device_code': code, 'connection_mode': settings['mode'], 'wait_ms': 5000}, observe=False)
            deadline = time.monotonic() + 120
            while not connected.get('connected') and time.monotonic() < deadline:
                connected = call('pab_connect', {'device_code': code, 'wait_ms': 5000}, observe=False)
            if not connected.get('connected'):
                raise RuntimeError('Test device did not connect')
            base = remote_base(device, run)
            host = report['hosts'][role] = {'code': code, 'base': str(base), 'created': False,
                'task': None, 'provisioned': False, 'launch_pending': False}
            made = call('pab_mkdir', {'device_code': code, 'path': str(base), 'parents': False, 'exist_ok': False})
            if made.get('result', {}).get('state') != 'completed':
                raise RuntimeError('Fresh lab directory was not created')
            host['created'] = True
            call('pab_file_write', {'device_code': code, 'path': str(base / 'owned-run.txt'), 'content': run, 'overwrite': False})
            config = f'mode = {json.dumps(settings["mode"])}\nrelay_urls = {json.dumps(settings["relay_urls"])}\n'
            call('pab_file_write', {'device_code': code, 'path': str(base / 'lab.toml'), 'content': config, 'overwrite': False})
            binary = base / ('native-batch.exe' if device['os'] == 'windows' else 'native-batch')
            transfer('pab_upload_file', code, Path(device['binary']).resolve(), binary, device['sha256'])
            remote_hash = call('pab_file_hash', {'device_code': code, 'path': str(binary)})
            if remote_hash.get('result', {}).get('metadata', {}).get('sha256') != device['sha256']:
                raise RuntimeError('Target binary hash mismatch')
            if device['os'] != 'windows':
                changed = call('pab_run_command', {'device_code': code, 'program': '/bin/chmod',
                               'args': ['0700', str(binary)], 'timeout_ms': 10000, 'wait_ms': 1})
                if changed.get('snapshot', changed.get('task', {})).get('state') != 'succeeded':
                    raise RuntimeError('Owned lab executable could not be made executable')
            host['binary'] = str(binary)
            host['os'] = device['os']
            host['provisioned'] = True
        for role, host in report['hosts'].items():
            env = {'PAB_FT_LAB_ROOT': host['base'], 'PAB_FT_LAB_RUN': run, 'PAB_FT_LAB_ROLE': role,
                   'TMP': host['base'], 'TEMP': host['base'], 'TMPDIR': host['base']}
            host['launch_pending'] = True
            host['launch_request'] = str(uuid.uuid4())
            save()
            started = call('pab_run_command', {'device_code': host['code'], 'request_id': host['launch_request'], 'program': host['binary'],
                           'args': ['--ignored', '--exact', TEST, '--nocapture'], 'cwd': host['base'],
                           'env': env, 'timeout_ms': 540000, 'wait_ms': 1}, observe=False)
            host['task'] = started['operation_ref']['task_id']
            host['operation_ref'] = started['operation_ref']
            save()
        for role, host in report['hosts'].items():
            deadline = time.monotonic() + 60
            expected = None
            while time.monotonic() < deadline:
                text = stdout(host)
                match = re.search(r'^OFFER .+ sha256=([0-9a-f]{64})$', text, re.M)
                if match:
                    expected = match[1]
                    break
                if task_done(host):
                    raise RuntimeError('Lab process exited before publishing its offer')
                time.sleep(.2)
            if expected is None:
                raise TimeoutError('Public offer was not published')
            style = PureWindowsPath if host['os'] == 'windows' else PurePosixPath
            local = output / f'{role}-offer.json'
            transfer('pab_download_file', host['code'], style(host['base']) / local.name, local.resolve(), expected)
            if sha256(local) != expected:
                raise RuntimeError('Received public offer hash mismatch')
            other = report['hosts']['server' if role == 'client' else 'client']
            other_style = PureWindowsPath if other['os'] == 'windows' else PurePosixPath
            transfer('pab_upload_file', other['code'], local.resolve(), other_style(other['base']) / local.name, expected)
        for role, host in report['hosts'].items():
            finished = client.observe({'operation_ref': host['operation_ref']}, timeout=360)
            report['hosts'][role]['finished'] = finished
            text = stdout(host)
            (output / f'{role}.stdout.log').write_text(text, encoding='utf-8')
            error = call('pab_read_output', {'device_code': host['code'], 'task_id': host['task'],
                         'stream': 'stderr', 'offset': 0, 'max_bytes': 16384}, observe=False)
            (output / f'{role}.stderr.log').write_text(error['text'], encoding='utf-8')
            snapshot = finished.get('snapshot', finished.get('task', {}))
            if snapshot.get('state') != 'succeeded' or '1 passed; 0 failed; 0 ignored' not in text:
                raise RuntimeError('Native lab did not pass')
            if role == 'client' and ('hash_match=true empty_directory=true' not in text
                    or settings['mode'] == 'relay' and 'PATH Some(Relay)' not in text):
                raise RuntimeError('Actual route or complete-file evidence is missing')
        report['status'] = 'pass'
    except BaseException as error:
        report['status'] = 'fail'
        report['error_type'] = type(error).__name__
        raise
    finally:
        for role, host in report['hosts'].items():
            cleanup = {'role': role, 'path': host['base'], 'complete': False}
            report['cleanup'].append(cleanup)
            try:
                if host['created'] and host['provisioned'] and (not host['launch_pending'] or host['task'] and task_done(host)):
                    # The unique directory was created by this run; no other
                    # source, installed data, or parent directory is removed.
                    removed = call('pab_file_delete', {'device_code': host['code'], 'path': host['base'],
                                   'recursive': True, 'max_depth': 64, 'max_entries': 4096, 'max_bytes': 1024 ** 3})
                    cleanup['complete'] = removed.get('result', {}).get('state') == 'completed'
            except Exception as error:
                cleanup['error_type'] = type(error).__name__
        if any(not item['complete'] for item in report['cleanup']):
            report['status'] = 'fail'
        save()
        (output / 'counts.json').write_text(json.dumps({'passed': int(report['status'] == 'pass'),
            'failed': int(report['status'] != 'pass'), 'skipped': 0}), encoding='utf-8')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--mcp', type=Path, required=True)
    parser.add_argument('--config', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args()
    settings = validate_lab(tomllib.loads(options.lab.read_text(encoding='utf-8-sig')))
    env = {key: value for key, value in os.environ.items() if not key.startswith('PAB_')}
    with McpClient(options.mcp.resolve(), options.output / 'mcp.log', config=options.config, env=env) as client:
        report = execute(client, settings, options.output)
    return int(report['status'] != 'pass')


if __name__ == '__main__':
    raise SystemExit(main())
