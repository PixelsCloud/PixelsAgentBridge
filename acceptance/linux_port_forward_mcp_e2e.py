"""Install the current Linux package in a disposable device and run TCP forwarding acceptance."""

import argparse
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / '.build/packages/pixels-agent-bridge-linux-x86_64-release.tar.gz'
FIXTURE = ROOT / 'acceptance/fixtures/port-forward-server.py'
CONTAINER = 'pab-port-forward-' + uuid.uuid4().hex[:10]
DATABASE = Path(os.environ.get('PAB_BRIDGE_DATABASE',
                               Path(os.environ['LOCALAPPDATA']) / 'PixelsAgentBridge/bridge.sqlite3'))


def run(*argv):
    result = subprocess.run(argv, capture_output=True, text=True, encoding='utf-8', errors='replace')
    if result.returncode:
        raise RuntimeError(f'{argv[0]} exited {result.returncode}: {result.stderr[-1500:]}')
    return result.stdout.strip()


def docker(*argv):
    return run('docker', *argv)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--relay-only', action='store_true')
    options = parser.parse_args()
    mcp = ROOT / ('target/debug/pab-mcp.exe' if options.relay_only
                  else 'target/release/pab-mcp.exe')
    if not all(path.is_file() for path in (PACKAGE, mcp, FIXTURE, DATABASE)):
        raise RuntimeError('Linux package, MCP, fixture or local Bridge database missing')
    device_id = None
    inserted = False
    started = False
    try:
        docker('run', '-d', '--rm', '--name', CONTAINER, '--privileged',
               '--cgroupns=private', '--tmpfs', '/run', '--tmpfs', '/run/lock', '--tmpfs', '/tmp',
               'pab-linux-headless-systemd-test', '/lib/systemd/systemd')
        started = True
        docker('cp', str(PACKAGE), f'{CONTAINER}:/root/pab-linux.tar.gz')
        docker('exec', CONTAINER, 'bash', '-lc',
               'mkdir -p /root/pab-package && tar -xzf /root/pab-linux.tar.gz -C /root/pab-package '
               '&& bash /root/pab-package/install.sh wss://pab.rgaa.vip/control https://pab-relay.rgaa.vip')
        assert docker('exec', CONTAINER, 'systemctl', 'is-active',
                      'pixels-agent-bridge-executor.service') == 'active'
        catalog_request = ''.join(json.dumps(message) + '\n' for message in (
            {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
                'protocolVersion': '2025-03-26', 'capabilities': {},
                'clientInfo': {'name': 'linux-port-forward-catalog', 'version': '1'}}},
            {'jsonrpc': '2.0', 'method': 'notifications/initialized'},
            {'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list'},
        ))
        catalog = subprocess.run(['docker', 'exec', '-i', CONTAINER,
                                  '/opt/pixels-agent-bridge/run-mcp.sh'],
                                 input=catalog_request, capture_output=True, text=True,
                                 timeout=30)
        if catalog.returncode:
            raise RuntimeError(f'installed Linux MCP failed: {catalog.stderr[-800:]}')
        replies = [json.loads(line) for line in catalog.stdout.splitlines() if line.startswith('{')]
        names = {item['name'] for item in next(reply for reply in replies if reply.get('id') == 2)
                 ['result']['tools']}
        required = {'pab_port_forward_open', 'pab_port_forward_status', 'pab_port_forward_close'}
        assert required <= names, required - names
        access_script = (
            "import json,sqlite3; c=sqlite3.connect('/var/lib/pixels-agent-bridge/executor.sqlite3'); "
            "r=c.execute('select tenant_id,device_id,device_code,temporary_password from device_access where id=1').fetchone(); "
            "print(json.dumps({'tenant_id':r[0],'device_id':r[1],'device_code':r[2],'password':r[3]}))"
        )
        for _ in range(24):
            try:
                access = json.loads(docker('exec', CONTAINER, 'python3', '-c', access_script))
                break
            except RuntimeError as error:
                if 'no such table: device_access' not in str(error):
                    raise
                time.sleep(2)
        else:
            raise RuntimeError('Linux Executor did not register within 48 seconds')
        device_id = access['device_id']
        with sqlite3.connect(DATABASE, timeout=10) as database:
            if database.execute('select 1 from device_credentials where device_id=?', (device_id,)).fetchone():
                raise RuntimeError('test credential already exists; refusing to overwrite')
            database.execute('insert into device_credentials(device_id,password) values (?,?)',
                             (device_id, access['password']))
            inserted = True
        del access['password']
        docker('cp', str(FIXTURE), f'{CONTAINER}:/root/port-forward-server.py')
        docker('exec', '-d', CONTAINER, 'python3', '/root/port-forward-server.py', '38673', '38674')
        for _ in range(20):
            try:
                docker('exec', CONTAINER, 'python3', '-c',
                       'import socket; socket.create_connection(("127.0.0.1",38673),1).close()')
                break
            except RuntimeError:
                time.sleep(.2)
        else:
            raise RuntimeError('Linux echo fixture did not start')
        result = subprocess.run([
            sys.executable, str(ROOT / 'acceptance/port_forward_live.py'), '--mcp', str(mcp),
            '--device-code', access['device_code'], '--os', 'linux',
            *(['--relay-only'] if options.relay_only else []),
        ], capture_output=True, text=True, timeout=180)
        if result.returncode:
            raise RuntimeError(f'Linux port forwarding failed: {result.stderr[-1500:]}\n{result.stdout[-1500:]}')
        print(json.dumps({'platform': 'linux-x86_64', 'path': 'relay' if options.relay_only else 'direct',
                          'device_code': access['device_code'], 'package_mcp_tools': sorted(required),
                          'container': CONTAINER, 'results': [json.loads(line)
                          for line in result.stdout.splitlines()]}))
    finally:
        if inserted and device_id:
            with sqlite3.connect(DATABASE, timeout=10) as database:
                database.execute('delete from device_credentials where device_id=?', (device_id,))
        if started:
            docker('stop', CONTAINER)


if __name__ == '__main__':
    main()
