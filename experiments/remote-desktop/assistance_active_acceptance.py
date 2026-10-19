"""Bounded real-process handoff acceptance against an already staged target.

Requires an isolated .build/remote-desktop profile with its own TOML config and
authorized device credential. Does not install software or send remote input.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import time
import uuid

import psutil
from mcp_client import McpClient

parser = argparse.ArgumentParser()
parser.add_argument('--profile', type=Path, required=True)
parser.add_argument('--code', required=True)
args = parser.parse_args()
workspace = Path(__file__).resolve().parents[2]
base = workspace / '.build/remote-desktop'
profile = args.profile.resolve()
assert base.resolve() in profile.parents and len(args.code) == 9 and args.code.isdecimal()
assert (profile / 'config.toml').is_file()
root = base / ('assistance-active-' + uuid.uuid4().hex[:12])
root.mkdir()
clients, requests = [], []
desktop = None
report = {'passed': False, 'remote_input_sent': False, 'phases': [],
          'artifacts': {n: hashlib.sha256((profile / n).read_bytes()).hexdigest()
                        for n in ('pab-desktop.exe', 'pab-mcp.exe')}}


def save():
    (root / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')


def phase(name, **value):
    report['phases'].append({'name': name, **value})
    save()
    print(json.dumps({'phase': name, **value}), flush=True)


def listener():
    with socket.socket() as sock:
        return sock.connect_ex(('127.0.0.1', 39100)) == 0


def eventually(fn, predicate, seconds=45):
    deadline = time.monotonic() + seconds
    while True:
        value = fn()
        if predicate(value):
            return value
        if time.monotonic() >= deadline:
            raise TimeoutError('Acceptance state not reached; inspect original result')
        time.sleep(.25)


def ui(action, token='', aid=''):
    raw = subprocess.check_output(['node', str(Path(__file__).with_suffix('.mjs')),
                                   action, token, aid], encoding='utf-8', timeout=60)
    return json.loads(raw)


def launch():
    global desktop
    assert desktop is None or desktop.poll() is not None
    assert not listener(), 'Another acceptance Desktop owns the debugger'
    env = dict(os.environ, PAB_DESKTOP_TERMINAL_ACCEPTANCE='1',
               WEBVIEW2_USER_DATA_FOLDER=str(profile / 'webview'))
    with (root / ('desktop-' + str(len(report['phases'])) + '.log')).open('wb') as log:
        desktop = subprocess.Popen([str(profile / 'pab-desktop.exe'), '--config',
                                    str(profile / 'config.toml')], cwd=profile,
                                   env=env, stdout=log, stderr=log,
                                   creationflags=subprocess.CREATE_NO_WINDOW)
    (profile / 'process.json').write_text(json.dumps({'pid': desktop.pid,
        'executable': str(profile / 'pab-desktop.exe')}))
    eventually(listener, bool, 25)
    return desktop.pid


def new_client():
    c = McpClient(profile / 'pab-mcp.exe')
    clients.append(c)
    return c


def status(c, request):
    return c.tool('pab_desktop_assistance', {'action': 'status',
        'assistance_id': request['assistance_id']})['assistance']


def request(c, token, ttl=300):
    r = c.tool('pab_desktop_assistance', {'action': 'request',
        'request_id': str(uuid.uuid4()), 'device_code': args.code,
        'reason': 'Acceptance: active lifecycle ' + token,
        'expected_action': 'View only; no remote input or automatic completion',
        'expires_in_seconds': ttl})['assistance']
    requests.append((c, r))
    return r


def active(c, r, token):
    ui('open', token)
    view = ui('ready')
    assert view['status']['phase'] == 'viewing' and view['status']['frame_ready']
    assert not view['status']['input_enabled']
    s = eventually(lambda: status(c, r), lambda s: s['status'] == 'active')
    assert s['window_label'] == view['label'] and s['outcome'] is None
    return s, view


def row(r):
    matches = []
    for db in (profile / 'data/operators').glob('*/desktop-assistance.sqlite3'):
        with sqlite3.connect(db.as_uri() + '?mode=ro', uri=True) as con:
            found = con.execute('SELECT payload FROM assistance_requests WHERE id=?',
                                (r['assistance_id'],)).fetchone()
        if found:
            matches.append(json.loads(found[0])['snapshot'])
    assert len(matches) == 1
    return matches[0]


def unchanged_view(previous):
    view = ui('unbound')
    assert view['label'] == previous['label']
    assert view['status']['phase'] == 'viewing' and view['status']['frame_ready']
    assert not view['status']['input_enabled']
    assert view['status']['opened']['config']['session_id'] == previous['status']['opened']['config']['session_id']
    return view


def stop_owned_desktop(crash=False):
    if desktop is None or desktop.poll() is not None:
        return
    children = psutil.Process(desktop.pid).children(recursive=True)
    desktop.kill() if crash else desktop.terminate()
    desktop.wait(timeout=10)
    _, alive = psutil.wait_procs(children, timeout=6)
    # Only owned media/WebView children; never terminate installed applications.
    for child in alive:
        try:
            exe = Path(child.exe()).resolve()
            cmd = child.cmdline()
            if (exe == profile / 'pab-desktop.exe' and '--media-worker' in cmd) or (
                    child.name().lower() == 'msedgewebview2.exe' and
                    str(profile / 'webview') in ' '.join(cmd)):
                child.terminate()
        except psutil.NoSuchProcess:
            pass
    eventually(listener, lambda value: not value, 10)


save()
print(json.dumps({'run': str(root), 'profile': str(profile)}), flush=True)
try:
    assert not listener()
    a = new_client()
    assert a.tool('pab_connect', {'device_code': args.code, 'wait_ms': 15000}).get('connected')
    launch()
    first = request(a, 'cancel')
    _, view = active(a, first, 'cancel')
    cancelled = a.tool('pab_desktop_assistance', {'action': 'cancel',
        'assistance_id': first['assistance_id']})['assistance']
    assert cancelled['status'] == 'cancelled' and cancelled['outcome'] == 'requester_cancelled'
    retained = unchanged_view(view)
    ui('stale-complete', '', first['assistance_id'])
    phase('active_tool_cancel', assistance=cancelled, viewer=retained, stale_completion_rejected=True)

    second = request(a, 'requester-crash')
    _, view = active(a, second, 'requester-crash')
    before = time.monotonic()
    a.process.kill()
    a.process.wait(timeout=10)
    ended = eventually(lambda: row(second), lambda s: s['status'] == 'cancelled')
    assert ended['outcome'] == 'requester_unavailable'
    phase('active_requester_crash', elapsed_seconds=time.monotonic()-before,
          assistance=ended, viewer=unchanged_view(view))

    b = new_client()
    third = request(b, 'desktop-crash')
    _, view = active(b, third, 'desktop-crash')
    old_pid, old_session = desktop.pid, view['status']['opened']['config']['session_id']
    before = time.monotonic()
    stop_owned_desktop(crash=True)
    new_pid = launch()
    ui('show', 'desktop-crash')
    resumed = eventually(lambda: status(b, third), lambda s:
        s['status'] == 'waiting' and s['delivery'] == 'displayed' and s['window_label'] is None)
    assert resumed['outcome'] is None
    ui('no-viewer')
    phase('active_desktop_crash_restart_waiting', old_pid=old_pid, new_pid=new_pid,
          elapsed_seconds=time.monotonic()-before, assistance=resumed, automatic_viewer=False)
    restored, view = active(b, third, 'desktop-crash')
    assert view['status']['opened']['config']['session_id'] != old_session
    phase('explicit_reopen_after_crash', assistance=restored, viewer=view, fresh_session=True)
    b.tool('pab_desktop_assistance', {'action': 'cancel', 'assistance_id': third['assistance_id']})
    unchanged_view(view)

    fourth = request(b, 'active-expiry', 15)
    _, view = active(b, fourth, 'active-expiry')
    expired = eventually(lambda: status(b, fourth), lambda s: s['status'] == 'expired', 20)
    assert expired['outcome'] == 'expired'
    phase('active_expiry', assistance=expired, viewer=unchanged_view(view))
    report['passed'] = True
except BaseException as e:
    report['failure'] = {'type': type(e).__name__, 'message': str(e)}
    raise
finally:
    for c, r in requests:
        if c.process.poll() is None:
            try:
                c.tool('pab_desktop_assistance', {'action': 'cancel', 'assistance_id': r['assistance_id']})
            except Exception as e:
                report.setdefault('cleanup_errors', []).append(type(e).__name__)
    for c in clients:
        c.close()
    stop_owned_desktop()
    report['owned_processes_exited'] = all(c.process.poll() is not None for c in clients) and (
        desktop is None or desktop.poll() is not None)
    report['debugger_released'] = not listener()
    save()
print(json.dumps({'passed': report['passed'], 'result': str(root/'result.json')}), flush=True)
