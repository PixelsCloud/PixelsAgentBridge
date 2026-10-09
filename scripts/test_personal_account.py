"""Real Executor IPC + account CLI acceptance against an isolated loopback server.

Start the development Web test server first. No installed service or user data is touched.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import sqlite3
import ssl
import subprocess
import time
import urllib.request
import uuid
from concurrent.futures import ThreadPoolExecutor
from queue import Queue
from threading import Thread

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, default=Path('target/debug'))
parser.add_argument('--certificate', type=Path, default=Path('.build/web-test/cert.pem'))
parser.add_argument('--origin', default='https://localhost:38443')
parser.add_argument('--output', type=Path, default=Path('.build/p1-client-acceptance'))
parser.add_argument('--mcp-count', type=int, default=2)
parser.add_argument('--relay-url', default='https://localhost:39443')
parser.add_argument('--relay-only', action='store_true')
args = parser.parse_args()
assert urllib.parse.urlsplit(args.origin).hostname in ('localhost', '127.0.0.1'), 'isolated loopback server required'
root = args.output.resolve() / uuid.uuid4().hex
root.mkdir(parents=True)
suffix = '.exe' if os.name == 'nt' else ''
executor = str((args.binaries / ('pab-executor' + suffix)).resolve())
mcp = str((args.binaries / ('pab-mcp' + suffix)).resolve())
with socket.socket() as listener:
    listener.bind(('127.0.0.1', 0))
    port = listener.getsockname()[1]
env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
env.update(PAB_DATA_DIR=str(root), PAB_CONTROL_URL=args.origin.replace('https:', 'wss:') + '/control',
           PAB_CONTROL_CA_CERT=str(args.certificate.resolve()), PAB_LOCAL_IPC_PORT=str(port),
           PAB_RELAY_URLS=args.relay_url, PAB_RELAY_CA_CERT=str(args.certificate.resolve()), PAB_DEVICE_NAME='P1 isolated IPC acceptance')
if args.relay_only:
    env['PAB_TEST_RELAY_ONLY'] = '1'
tls = ssl.create_default_context(cafile=str(args.certificate.resolve()))
process = None
log = (root / 'executor-test.log').open('w', encoding='utf-8')
password = uuid.uuid4().hex
accounts = [f'p1-native-{uuid.uuid4().hex[:14]}' for _ in range(2)]

def cli(*arguments, password_input=False, environment=None):
    result = subprocess.run([mcp, 'account', *arguments], input=password + '\n' if password_input else None,
                            env=environment or env, cwd=root, text=True, capture_output=True, timeout=60)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)

def http(method, path, body=None, token=None, cookie=None):
    headers = {'Content-Type': 'application/json', 'Origin': args.origin}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    if cookie:
        headers['Cookie'] = cookie
    request = urllib.request.Request(args.origin + path, method=method, headers=headers,
                                     data=None if body is None else json.dumps(body).encode())
    with urllib.request.urlopen(request, context=tls, timeout=20) as response:
        raw = response.read()
        return (json.loads(raw) if raw else None), response.headers.get('Set-Cookie')

def identity():
    with sqlite3.connect(root / 'executor.sqlite3') as database:
        return database.execute('SELECT device_id,device_code FROM device_access LIMIT 1').fetchone()

def start():
    global process
    process = subprocess.Popen([executor], env=env, cwd=root, stdout=log, stderr=log,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        assert process.poll() is None, 'test Executor exited; see isolated log'
        try:
            if identity():
                with socket.create_connection(('127.0.0.1', port), timeout=1):
                    return
        except (sqlite3.Error, OSError):
            pass
        time.sleep(.2)
    raise AssertionError('test Executor not ready')

def stop():
    if process and process.poll() is None:
        process.terminate()
        process.wait(timeout=15)

report = {'passed': False, 'platform': os.name, 'cases': []}
children = []

class Mcp:
    def __init__(self, environment=None):
        self.process = subprocess.Popen([mcp], env=environment or env, cwd=root, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=log, text=True,
                                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
        children.append(self.process)
        self.queue = Queue()
        self.sequence = 0
        def read():
            for line in self.process.stdout:
                self.queue.put(json.loads(line))
        Thread(target=read, daemon=True).start()
        self.rpc('initialize', {'protocolVersion': '2025-03-26', 'capabilities': {},
                               'clientInfo': {'name': 'P1 isolated acceptance', 'version': '1'}})
        self.process.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n')
        self.process.stdin.flush()

    def rpc(self, method, params):
        self.sequence += 1
        self.process.stdin.write(json.dumps({'jsonrpc':'2.0','id':self.sequence,'method':method,'params':params})+'\n')
        self.process.stdin.flush()
        while True:
            result = self.queue.get(timeout=150)
            if result.get('id') == self.sequence:
                assert 'error' not in result, result
                return result['result']

    def tool(self, name, arguments):
        result = self.rpc('tools/call', {'name':name,'arguments':arguments})
        assert not result.get('isError'), result
        return result.get('structuredContent') or json.loads(result['content'][0]['text'])

    def connect(self, code):
        for _ in range(6):
            result = self.tool('pab_connect', {'device_code':code,'wait_ms':30000})
            if result.get('connected'):
                return self
        raise AssertionError('isolated MCP connection did not complete')

try:
    start()
    original = identity()
    a = cli('register', accounts[0], '--password-stdin', password_input=True)
    assert a['device_association']['status'] == 'associated', a
    assert identity() == original
    _, cookie_a = http('POST', '/api/web/session', {'username': accounts[0], 'password': password})
    assert http('GET', '/api/web/devices', cookie=cookie_a)[0]['total'] == 1
    report['cases'].append('HTTP registration + protected IPC proof + personal Web visibility')
    cli('logout')
    assert http('GET', '/api/web/devices', cookie=cookie_a)[0]['total'] == 1
    b = cli('register', accounts[1], '--password-stdin', password_input=True)
    changed = b['device_association']
    assert changed['status'] == 'associated' and changed['revision'] == 2, changed
    assert http('GET', '/api/web/devices', cookie=cookie_a)[0]['total'] == 0
    assert identity() == original
    report['cases'].append('sign-out retains association; login automatically changes only this device account and preserves identity')
    native, _ = http('POST', '/api/account/session', {'username': accounts[1], 'password': password})
    unlink, _ = http('DELETE', f'/api/account/devices/{original[0]}/association', {'revision': 2}, native['access_token'])
    assert unlink['status'] == 'unlinked'
    stop()
    start()
    b = cli('login', accounts[1], '--password-stdin', password_input=True)
    assert b['device_association']['status'] == 'associated', b
    assert identity() == original
    assert cli('associate')['revision'] == 4
    report['cases'].append('new login automatically restores Web-unlinked device; repeated association is idempotent')
    cli('devices', 'status')  # Initialize only this test's Bridge store.
    with sqlite3.connect(root / 'executor.sqlite3') as database:
        device_id, code, device_password = database.execute('SELECT device_id,device_code,temporary_password FROM device_access').fetchone()
    with sqlite3.connect(root / 'bridge.sqlite3') as database:
        database.execute('INSERT OR REPLACE INTO device_credentials(device_id,password) VALUES(?,?)', (device_id, device_password))
    code = str(code)
    with ThreadPoolExecutor(max_workers=args.mcp_count) as pool:
        clients = list(pool.map(lambda _: Mcp().connect(code), range(args.mcp_count)))
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        saved = http('GET', '/api/account/saved-devices', token=native['access_token'])[0]
        if any(item['device_ref']['device_id'] == device_id for item in saved['items']):
            break
        time.sleep(.5)
    else:
        raise AssertionError('authenticated target did not save the remote device')
    report['cases'].append(f'{args.mcp_count} real MCP stdio processes connect; target proof automatically saves remote device')
    _, cookie_b = http('POST', '/api/web/session', {'username': accounts[1], 'password': password})
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        usage = http('GET', '/api/web/usage', cookie=cookie_b)[0]
        if usage['counters']['connections'] >= args.mcp_count:
            break
        time.sleep(1)
    else:
        raise AssertionError('client connection counters were not uploaded')
    report['cases'].append('real MCP connection counters reach personal Web usage')
    # A second independent profile shares only the HTTP account, never the SQLite cache or device password.
    second_root = root / 'second-client'
    second_root.mkdir()
    second_env = dict(env, PAB_DATA_DIR=str(second_root))
    cli('login', accounts[1], '--password-stdin', password_input=True, environment=second_env)
    cli('devices', 'sync', environment=second_env)
    second = Mcp(second_env)
    assert any(str(item['device_code']) == code for item in second.tool('pab_list_devices', {})['devices'])
    assert http('GET', '/api/web/devices', cookie=cookie_b)[0]['total'] == 1
    saved_item = next(item for item in saved['items'] if item['device_ref']['device_id'] == device_id)
    http('POST', '/api/account/saved-devices', {'id':str(uuid.uuid4()), 'device_id':device_id,
         'expected_revision':saved_item['revision'],'alias':'second-client alias','deleted':False}, token=native['access_token'])
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        cli('devices', 'sync', environment=second_env)
        names = [item['name'] for item in second.tool('pab_list_devices', {})['devices']]
        first_names = [item['name'] for item in clients[0].tool('pab_list_devices', {})['devices']]
        if 'second-client alias' in names and 'second-client alias' in first_names:
            break
        time.sleep(2)
    else:
        raise AssertionError('independent client catalogs did not converge')
    report['cases'].append('two independent client profiles converge on saved device and personal alias without sharing credentials')
    source, destination = root / 'upload-test.bin', root / 'remote-copy.bin'
    source.write_bytes(os.urandom(4096))
    upload = clients[0].tool('pab_upload_file', {'device_code':code,'source':str(source),
                           'destination':str(destination),'overwrite':False,'request_id':str(uuid.uuid4()),'wait_ms':5000})
    operation_id = upload['operation_ref']['operation_id']
    for _ in range(8):
        result = clients[0].tool('pab_get_operation', {'device_code':code,'operation_id':operation_id,'wait_until':'complete','wait_ms':10000})
        if result.get('complete'):
            assert result['operation']['state'] == 'completed', result
            break
    else:
        raise AssertionError('isolated upload did not complete')
    assert destination.read_bytes() == source.read_bytes()
    deadline = time.monotonic() + 75
    while time.monotonic() < deadline:
        usage = http('GET', '/api/web/usage', cookie=cookie_b)[0]
        if usage['counters']['uploaded_files'] == 1:
            assert usage['counters']['uploaded_bytes'] == 4096
            break
        time.sleep(1)
    else:
        raise AssertionError('file totals were not uploaded')
    report['cases'].append('real 4096-byte binary upload verified and counted once without uploading its path')
    if args.relay_only:
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            usage = http('GET', '/api/web/usage', cookie=cookie_b)[0]
            if usage['counters']['relay_upload_bytes'] > 0 and usage['counters']['relay_download_bytes'] > 0:
                break
            time.sleep(1)
        else:
            raise AssertionError('actual Relay forwarding counters did not reach the user account')
        report['cases'].append('Relay-only encrypted traffic is durably reported in both directions to the verified user')
    second.process.stdin.close()
    assert second.process.wait(timeout=20) == 0
    cli('logout', environment=second_env)
    for client in clients:
        client.tool('pab_disconnect', {'device_code':code})
        client.process.stdin.close()
        assert client.process.wait(timeout=20) == 0
    cli('logout')
    report['passed'] = True
finally:
    for child in children:
        if child.poll() is None:
            child.terminate()
            child.wait(timeout=20)
    stop()
    log.close()
    (root / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps({'result': str(root / 'result.json'), **report}))
