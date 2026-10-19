"""Public build smoke test: isolated service, Desktop, MCP; no installation/network target.

Uses fresh machine/user roots and an owned loopback IPC listener. Never copies an
account or credential, changes the installed app, or contacts a production server.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import tomllib
import uuid

import psutil
from mcp_client import McpClient

workspace = Path(__file__).resolve().parents[2]
base = workspace / '.build/remote-desktop'
root = base / ('config-public-smoke-' + uuid.uuid4().hex[:12])
root.mkdir()
identity = subprocess.check_output(['whoami'], text=True).strip()
subprocess.run(['icacls', str(root), '/inheritance:r', '/grant:r',
                identity + ':(OI)(CI)F', '*S-1-5-18:(OI)(CI)F'], check=True, capture_output=True)
report = {'passed': False, 'production_network_used': False, 'installed_app_changed': False, 'phases': []}
processes = []


def save():
    (root / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def listening(port):
    with socket.socket() as sock:
        return sock.connect_ex(('127.0.0.1', port)) == 0


def wait(fn, timeout=25):
    end = time.monotonic() + timeout
    while not fn():
        if time.monotonic() >= end:
            raise TimeoutError('Isolated startup state not reached')
        time.sleep(.2)


def launch(name):
    with (root / (name + '-' + str(len(processes)) + '.log')).open('wb') as log:
        proc = subprocess.Popen([str(root / (name + '.exe'))], cwd=root, env=environment,
                                stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
    processes.append(proc)
    return proc


def stop(proc):
    if proc.poll() is not None:
        return
    children = psutil.Process(proc.pid).children(recursive=True)
    proc.terminate()
    proc.wait(timeout=10)
    _, alive = psutil.wait_procs(children, timeout=5)
    for child in alive:
        try:
            exe, cmd = Path(child.exe()).resolve(), ' '.join(child.cmdline())
            if (root in exe.parents) or (child.name().lower() == 'msedgewebview2.exe' and str(root / 'webview') in cmd):
                child.terminate()
                child.wait(timeout=5)
        except psutil.NoSuchProcess:
            pass


print(json.dumps({'run': str(root)}), flush=True)
try:
    names = ['pab-executor', 'pab-desktop', 'pab-mcp']
    for name in names:
        shutil.copy2(base / 'desktop-check/debug' / (name + '.exe'), root)
    report['artifacts'] = {name: hashlib.sha256((root / (name + '.exe')).read_bytes()).hexdigest() for name in names}
    config = 'control_url = ""\nrelay_urls = []\n[executor]\ndata_dir = "machine"\nlog_dir = "machine/logs"\n[operator]\ndata_dir = "user"\nlog_dir = "user/logs"\n'
    (root / 'config.toml').write_text(config, encoding='utf-8')
    ipc, cdp = free_port(), free_port()
    while cdp == ipc:
        cdp = free_port()
    environment = dict(os.environ, PAB_LOCAL_IPC_PORT=str(ipc), PAB_DESKTOP_TERMINAL_ACCEPTANCE='1',
                       WEBVIEW2_USER_DATA_FOLDER=str(root / 'webview'))
    # Never inherit another session's credentials, URLs, or restart snapshots.
    for key in list(environment):
        if key.startswith('PAB_') and key not in ('PAB_LOCAL_IPC_PORT', 'PAB_DESKTOP_TERMINAL_ACCEPTANCE'):
            environment.pop(key)
    environment['PAB_UPDATE_SITE_URL'] = 'https://127.0.0.1:39493'
    executor = launch('pab-executor')
    wait(lambda: executor.poll() is not None or listening(ipc))
    assert executor.poll() is None
    subprocess.run([str(root / 'pab-executor.exe'), 'issue-local-access', str(root / 'user/local-access.key')],
                   cwd=root, env=environment, check=True, capture_output=True, timeout=15)
    for phase in ('empty', 'customer', 'cleared'):
        with (root / ('desktop-' + phase + '.log')).open('wb') as log:
            desktop = subprocess.Popen([str(root / 'pab-desktop.exe'), f'--acceptance-cdp-port={cdp}'],
                                        cwd=root, env=environment, stdout=log, stderr=log,
                                        creationflags=subprocess.CREATE_NO_WINDOW)
        processes.append(desktop)
        wait(lambda: desktop.poll() is not None or listening(cdp))
        assert desktop.poll() is None
        raw = subprocess.check_output(['node', str(Path(__file__).with_suffix('.mjs')), str(cdp), phase],
                                      encoding='utf-8', timeout=65)
        report['phases'].append(json.loads(raw))
        stop(desktop)
        wait(lambda: not listening(cdp), 10)
        assert (root / 'config.toml').read_text(encoding='utf-8') == config
        user_config = tomllib.loads((root / 'user/operator-server.toml').read_text(encoding='utf-8'))
        assert bool(user_config['controlUrl']) == (phase == 'empty')
        save()
    # Explicit config keeps MCP on this new, credential-free user root.
    with McpClient(root / 'pab-mcp.exe', ['--config', root / 'config.toml']) as mcp:
        assert mcp.rpc('tools/list', {}).get('tools')
        report['mcp_initialized_without_servers'] = True
    assert executor.poll() is None
    assert not (root / 'machine/executor.sqlite3').exists()
    assert not (root / 'machine/device-endpoint.key').exists()
    report['empty_background_alive_without_device_identity'] = True
    report['passed'] = True
finally:
    for proc in reversed(processes):
        stop(proc)
    report['owned_processes_stopped'] = all(p.poll() is not None for p in processes)
    save()
    print(json.dumps(report), flush=True)
if not report['passed']:
    sys.exit(1)
