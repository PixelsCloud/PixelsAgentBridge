"""Test Docker tools against one isolated, labeled, resource-limited container.

The selected local Docker context must expose the same daemon as the test
device. An existing image with /bin/sh and sleep is required; nothing is pulled.
No host ports, bind mounts, volumes or existing containers are changed.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import time
import uuid

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('config', 'mcp', 'output'):
        parser.add_argument('--' + key, type=Path, required=True)
    for key in ('docker-context', 'image', 'device-code'):
        parser.add_argument('--' + key, required=True)
    args = parser.parse_args()
    token = str(uuid.uuid4())
    name, label = 'pab-smoke-' + token, 'com.pixels.smoke.run=' + token
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'calls': [], 'checks': [], 'cleanup': False,
              'fixture': {'name': name, 'label': label, 'image': args.image, 'docker_context': args.docker_context}}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    def docker(*values, check=True):
        return subprocess.run(['docker', '--context', args.docker_context, *values],
            check=check, capture_output=True, text=True, encoding='utf-8', timeout=30)

    def inspect(identifier):
        return json.loads(docker('inspect', identifier).stdout)[0]

    image = json.loads(docker('image', 'inspect', args.image).stdout)[0]
    assert not image['Config'].get('Volumes'), 'Choose a fixture image without declared volumes'
    report['fixture']['image_id'] = image['Id']
    cid = None
    save()
    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    try:
        with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as c:
            def call(tool, values):
                entry = {'tool': tool, 'arguments': values, 'value': None}
                report['calls'].append(entry)
                save()
                try:
                    value = c.call(tool, values)
                    entry['value'] = value
                    save()
                    value = c.observe(value)
                except McpToolError as error:
                    entry['value'] = error.value
                    save()
                    raise
                entry['value'] = value
                save()
                assert not value.get('error') and value.get('result', {}).get('state') not in ('failed', 'cancelled'), tool
                return value

            def check(name):
                report['checks'].append({'name': name, 'passed': True})
                save()
                print(name + ': passed', flush=True)

            code = args.device_code
            connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
            deadline = time.monotonic() + 50
            while connected.get('state') == 'connecting' and time.monotonic() < deadline:
                connected = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
            assert connected.get('connected')
            cid = docker('create', '--pull=never', '--name', name, '--label', label,
                '--network=none', '--memory=64m', '--cpus=0.25', '--pids-limit=16', '--restart=no',
                '--entrypoint=/bin/sh', image['Id'], '-c', "trap 'exit 0' TERM;echo " + name + ";sleep 120 & wait $!").stdout.strip()
            assert len(cid) == 64
            report['fixture']['container_id'] = cid
            save()

            def snapshot(tool, **values):
                return call(tool, {'device_code': code, **values})['result']['data']['snapshot']

            observed = snapshot('pab_get_container', container=cid)
            assert observed['container']['id'] == cid, 'Docker context and test device do not share the fixture daemon'
            assert not observed['container']['state']['running']
            listed = snapshot('pab_list_containers', all=True, name=name, labels=[label])
            assert len(listed['entries']) == 1 and listed['entries'][0]['id'] == cid
            check('exact_id_inventory_and_owned_label_filter')
            started = snapshot('pab_container_control', container=cid, control='start', timeout_ms=10000)
            assert started['daemon_acknowledged'] and started['desired_state_observed']
            before = inspect(cid)
            assert before['State']['Running']
            logs = snapshot('pab_container_logs', container=cid, tail=20)
            assert name in ''.join(row['text'] for row in logs['logs'])
            check('start_native_running_state_and_real_log_content')
            restarted = snapshot('pab_container_control', container=cid, control='restart', timeout_ms=10000, stop_timeout_seconds=2)
            assert restarted['daemon_acknowledged'] and restarted['desired_state_observed']
            after = inspect(cid)
            assert after['State']['Running'] and after['State']['StartedAt'] != before['State']['StartedAt']
            stopped = snapshot('pab_container_control', container=cid, control='stop', timeout_ms=10000, stop_timeout_seconds=2)
            assert stopped['daemon_acknowledged'] and stopped['desired_state_observed']
            assert not inspect(cid)['State']['Running']
            check('restart_observed_new_start_time_and_stop_confirmed')
            call('pab_disconnect', {'device_code': code})
            report['status'] = 'passed'
    except BaseException as error:
        report['status'] = 'failed'
        report['error'] = repr(error)
        raise
    finally:
        # Recover a creation with an uncertain CLI result using the unique exact name.
        owned = docker('inspect', cid or name, check=False)
        if owned.returncode == 0:
            current = json.loads(owned.stdout)[0]
            assert current['Name'] == '/' + name and current['Config']['Labels'].get('com.pixels.smoke.run') == token
            assert current['Image'] == image['Id'] and not current['Mounts'], 'fixture ownership changed'
            docker('rm', '-f', current['Id'])
            assert docker('inspect', current['Id'], check=False).returncode != 0
        report['cleanup'] = True
        save()


if __name__ == '__main__':
    main()
