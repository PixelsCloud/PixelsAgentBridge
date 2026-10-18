"""Bounded product video + command/PTY/file mixed regression.

Requires a private Desktop/MCP profile, an already-staged Linux candidate with
an independent rollback, and linux_display_fixture.py on the active test GUI.
This driver does not deploy candidates or change installed configuration.
"""
import argparse
import ctypes as C
from ctypes import wintypes as W
import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import time
import uuid
import psutil
from mcp_client import McpClient

p = argparse.ArgumentParser()
p.add_argument('--profile', type=pathlib.Path, required=True)
p.add_argument('--remote-run', type=pathlib.Path, required=True)
p.add_argument('--ssh-helper', type=pathlib.Path, required=True)
p.add_argument('--code', required=True)
p.add_argument('--seconds', type=int, default=1800)
a = p.parse_args()
assert len(a.code) == 9 and a.code.isdecimal() and 60 <= a.seconds <= 7200
root = a.profile.resolve(strict=True)
base = pathlib.Path(__file__).resolve().parent
operation = json.loads((a.remote_run / 'operation.json').read_text())
stage, fixture = operation['stage'], operation['fixture']
assert pathlib.PurePosixPath(stage).parent == pathlib.PurePosixPath('/tmp')
assert pathlib.PurePosixPath(stage).name.startswith('pab-formal-linux-')
owner = psutil.Process(json.loads((root / 'process.json').read_text())['pid'])
assert pathlib.Path(owner.exe()).resolve() == root / 'pab-desktop.exe'
spec = importlib.util.spec_from_file_location('private_ssh', a.ssh_helper.resolve())
ssh = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ssh)
report = {'requested_seconds': a.seconds, 'device_code': a.code, 'rounds': [],
          'artifacts': {n: hashlib.sha256((root/n).read_bytes()).hexdigest() for n in ('pab-desktop.exe', 'pab-mcp.exe')},
          'remote_artifacts': operation['hashes']}
result_path = root / 'mixed-result.json'
assert not result_path.exists(), 'Preserve the previous run; do not replay unconfirmed operations'

def save():
    result_path.write_text(json.dumps(report, indent=2), encoding='utf-8')

def ui(action):
    r = subprocess.run(['node', str(base/'viewer_reopen_cdp.mjs'), action, a.code],
                       capture_output=True, text=True, timeout=40)
    assert r.returncode == 0, r.stderr[-1000:]
    return json.loads(r.stdout)

u = C.WinDLL('user32', use_last_error=True)
callback = C.WINFUNCTYPE(W.BOOL, W.HWND, W.LPARAM)
u.SetProcessDpiAwarenessContext.argtypes = [W.HANDLE]
u.SetProcessDpiAwarenessContext(W.HANDLE(-4))
u.EnumWindows.argtypes = [callback, W.LPARAM]
u.GetWindowThreadProcessId.argtypes = [W.HWND, C.POINTER(W.DWORD)]
u.GetWindowTextW.argtypes = [W.HWND, W.LPWSTR, C.c_int]
u.GetWindowLongW.argtypes = [W.HWND, C.c_int]
u.SetWindowPos.argtypes = [W.HWND, W.HWND, C.c_int, C.c_int, C.c_int, C.c_int, W.UINT]
u.PostMessageW.argtypes = [W.HWND, W.UINT, W.WPARAM, W.LPARAM]

def windows():
    found = []
    @callback
    def visit(h, _):
        pid = W.DWORD()
        u.GetWindowThreadProcessId(h, C.byref(pid))
        if pid.value == owner.pid:
            title = C.create_unicode_buffer(256)
            u.GetWindowTextW(h, title, 256)
            if a.code in title.value.replace(' ', ''):
                found.append(h)
        return True
    u.EnumWindows(visit, 0)
    return found

def until(fn, seconds=20):
    end = time.monotonic() + seconds
    while True:
        value = fn()
        if value:
            return value
        assert owner.is_running() and time.monotonic() < end, 'Observation timeout; inspect the original operation'
        time.sleep(.1)

def ready():
    trace = ui('trace')
    if trace is None:
        return None
    report['status_trace'] = trace
    s = trace['current']
    report['last_status'] = s
    assert not s.get('error'), s
    return s if s['phase'] == 'viewing' and s['frame_ready'] else None


def unchanged_view_only(status, expected_session):
    observed = status['opened']['config']['session_id']
    assert observed == expected_session, f'Video session changed: {expected_session} -> {observed}'
    assert not status['input_enabled'], 'Input enabled during view-only mixed test'

def close_viewer():
    found = windows()
    if not found:
        return
    assert len(found) == 1
    assert u.PostMessageW(found[0], 0x10, 0, 0)
    until(lambda: not windows() and ui('closed'))

def local_resources():
    rows = []
    for proc in [owner, *owner.children(recursive=True)]:
        try:
            info = proc.memory_info()
            cpu = proc.cpu_times()
            rows.append({'pid': proc.pid, 'name': proc.name(), 'rss': info.rss,
                         'private': info.private, 'handles': proc.num_handles(),
                         'threads': proc.num_threads(), 'cpu_seconds': cpu.user + cpu.system})
        except psutil.NoSuchProcess:
            pass
    return rows

remote_script = '''import json,pathlib,time,os
stage=pathlib.Path(STAGE);fixture=pathlib.Path(FIXTURE)
assert stage.is_dir() and not stage.is_symlink()
rows=[]
for p in pathlib.Path('/proc').iterdir():
 if not p.name.isdecimal():continue
 try:
  if (p/'exe').resolve().parent!=stage:continue
  args=(p/'cmdline').read_bytes().split(b'\\0')[1:]
  mode=next((x.decode() for x in args if x in (b'--media-supervisor',b'--media-helper',b'--media-worker')),'executor')
  worker=args[args.index(b'--media-worker')+1].decode() if b'--media-worker' in args else None
  status=dict(x.split(':',1) for x in (p/'status').read_text().splitlines() if ':' in x)
  stat=(p/'stat').read_text();fields=stat[stat.rfind(')')+2:].split()
  rows.append(dict(pid=int(p.name),mode=mode,worker=worker,fds=len(list((p/'fd').iterdir())),rss_kib=int(status.get('VmRSS','0 kB').split()[0]),threads=int(status['Threads']),cpu_ticks=int(fields[11])+int(fields[12])))
 except (OSError,ProcessLookupError):pass
animation=json.loads((fixture/'animation.json').read_text())
assert time.time()-animation['updated']<2
assert not (stage/'state/restored.json').exists()
system=dict(cpu_count=os.cpu_count(),stat=pathlib.Path('/proc/stat').read_text(),load=pathlib.Path('/proc/loadavg').read_text())
system['memory']={k:v.strip() for k,v in (line.split(':',1) for line in pathlib.Path('/proc/meminfo').read_text().splitlines()) if k in ('MemTotal','MemAvailable','SwapTotal','SwapFree','Dirty')}
system['pressure']={p.name:p.read_text() for p in pathlib.Path('/proc/pressure').iterdir() if p.name in ('cpu','memory','io')}
print(json.dumps(dict(processes=rows,animation=animation,cpu_tick_hz=os.sysconf('SC_CLK_TCK'),system=system)))
'''.replace('STAGE', repr(stage)).replace('FIXTURE', repr(fixture))
client, password = ssh.connect()
source = root / 'mixed-payload.bin'
destination = root / 'mixed-download.bin'
source.write_bytes(os.urandom(4 * 1024 * 1024))
digest = hashlib.sha256(source.read_bytes()).hexdigest()
remote_file = stage + '/mixed-payload.bin'
terminal_id = None
record_serial = 0

def tool(c, name, args):
    global record_serial
    record_serial += 1
    path = root / f'mixed-operation-{record_serial:04d}.json'
    # Persist the request ID before submission. Timeouts never trigger replay.
    path.write_text(json.dumps({'tool': name, 'arguments': args}), encoding='utf-8')
    reply = c.tool(name, args)
    path.write_text(json.dumps({'tool': name, 'arguments': args, 'reply': reply}), encoding='utf-8')
    return reply

def transfer(c, name, src, dst):
    result = tool(c, name, {'device_code': a.code, 'request_id': str(uuid.uuid4()),
                          'source': src, 'destination': dst, 'overwrite': True, 'wait_ms': 5000})
    oid = result['operation_ref']['operation_id']
    for _ in range(12):
        result = tool(c, 'pab_get_operation', {'device_code': a.code, 'operation_id': oid,
                                             'wait_until': 'complete', 'wait_ms': 5000})
        if result.get('complete'):
            assert result['operation']['state'] == 'completed', result
            return oid
    raise RuntimeError('Transfer still pending: ' + oid)

def command(c, script):
    result = tool(c, 'pab_run_command', {'device_code': a.code, 'request_id': str(uuid.uuid4()),
        'program': '/usr/bin/python3', 'args': ['-c', script], 'wait_ms': 1000, 'timeout_ms': 15000})
    tid = result['operation_ref']['task_id']
    result = tool(c, 'pab_get_task', {'device_code': a.code, 'task_id': tid, 'wait_until': 'complete', 'wait_ms': 20000})
    assert result.get('complete') and result['snapshot']['state'] == 'succeeded', result
    output = tool(c, 'pab_read_output', {'device_code': a.code, 'task_id': tid, 'stream': 'stdout', 'offset': 0})
    return tid, output['text']

def terminal(c, number):
    marker = f'__PAB_MIXED_{number:04d}__'
    # The complete marker never occurs in echoed input.
    data = "printf '%s%s\\n' '__PAB_MIXED_' '" + f'{number:04d}' + "__'\r"
    tool(c, 'pab_terminal_input', {'session_id': terminal_id, 'data': data})
    text = ''
    end = time.monotonic() + 10
    while marker not in text:
        chunk = tool(c, 'pab_terminal_read', {'session_id': terminal_id})
        assert not chunk.get('ended'), chunk
        text += chunk.get('data', chunk.get('text', ''))
        assert time.monotonic() < end, 'PTY marker not received'
        if marker not in text:
            time.sleep(.1)
    return marker

try:
    with McpClient(root/'pab-mcp.exe') as c:
        connected = tool(c, 'pab_connect', {'device_code': a.code, 'wait_ms': 15000})
        assert connected.get('connected')
        assert not windows(), 'Do not reuse a previous unverified viewer'
        assert ui('open')['accepted']
        status = until(ready)
        session = status['opened']['config']['session_id']
        assert status['opened']['config']['screen']['pixel_width'] == 1920
        assert status['opened']['config']['screen']['pixel_height'] == 1080
        found = windows(); assert len(found) == 1
        assert u.SetWindowPos(found[0], None, 20, 40, 720, 600, 0x14)
        report['session_id'] = session
        opened = tool(c, 'pab_open_terminal', {'device_code': a.code})
        terminal_id = opened['session_id']
        report['terminal_session_id'] = terminal_id
        start = time.monotonic()
        round_number = 0
        while time.monotonic() - start < a.seconds:
            round_number += 1
            round_start = time.monotonic()
            status = until(ready)
            report['last_status'] = status
            unchanged_view_only(status, session)
            assert status['decoder'] == 'd3d11_hardware'
            row = {'index': round_number, 'elapsed': round_start-start, 'passed': False}
            row['remote_before'] = json.loads(ssh.run(client, password, remote_script, 10))
            report['rounds'].append(row); save()
            # Sample actual presentations WHILE old business runs, not afterwards.
            found = windows(); assert len(found) == 1
            was_top = bool(u.GetWindowLongW(found[0], -20) & 8)
            assert u.SetWindowPos(found[0], W.HWND(-1), 0, 0, 0, 0, 0x13)
            sample_path = root / f'mixed-presentation-{round_number:03d}.json'
            with (root/f'mixed-presentation-{round_number:03d}.log').open('wb') as log:
                sampler = subprocess.Popen([sys.executable, str(base/'windows_sample_presentation.py'),
                    str(root), '--seconds', '10', '--output', str(sample_path)], stdout=log, stderr=log,
                    creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                row['upload'] = transfer(c, 'pab_upload_file', str(source), remote_file)
                row['command'], output = command(c, "import pathlib,hashlib; p=pathlib.Path(" + repr(remote_file) + "); assert hashlib.sha256(p.read_bytes()).hexdigest()==" + repr(digest) + "; print('mixed-round-" + str(round_number) + "')")
                assert output.strip() == 'mixed-round-' + str(round_number)
                row['terminal_marker'] = terminal(c, round_number)
                row['download'] = transfer(c, 'pab_download_file', remote_file, str(destination))
                assert hashlib.sha256(destination.read_bytes()).hexdigest() == digest
                sample_status = sampler.wait(timeout=20)
                row['local'] = local_resources()
                row['remote'] = json.loads(ssh.run(client, password, remote_script, 10))
                if sample_path.exists():
                    row['presentation'] = json.loads(sample_path.read_text())
                assert sample_status == 0, 'Inspect actual presentation sample'
                assert row['presentation']['passed']
            finally:
                if sampler.poll() is None:
                    sampler.terminate(); sampler.wait(timeout=5)
                assert u.SetWindowPos(found[0], W.HWND(-1 if was_top else -2), 0, 0, 0, 0, 0x13)
            assert any(x['worker'] == 'capture' for x in row['remote']['processes'])
            status = until(ready)
            report['last_status'] = status
            unchanged_view_only(status, session)
            row['passed'] = True; save()
            print(json.dumps({'round': round_number, 'elapsed': time.monotonic()-start,
                              'actual_fps': row['presentation']['steady_fps']}), flush=True)
            while time.monotonic()-round_start < 30 and time.monotonic()-start < a.seconds:
                assert owner.is_running(); time.sleep(.25)
        report['elapsed_seconds'] = time.monotonic()-start
        close_viewer()
        report['terminal_after_video_close'] = terminal(c, 9999)
        report['command_after_video_close'], text = command(c, "print('after-video-close')")
        assert text.strip() == 'after-video-close'
        report['upload_after_video_close'] = transfer(c, 'pab_upload_file', str(source), remote_file)
        report['download_after_video_close'] = transfer(c, 'pab_download_file', remote_file, str(destination))
        assert hashlib.sha256(destination.read_bytes()).hexdigest() == digest
        tool(c, 'pab_terminal_close', {'session_id': terminal_id}); terminal_id = None
        report['remove_test_payload'], text = command(c, "import pathlib; p=pathlib.Path(" + repr(remote_file) + "); p.unlink(); print('removed')")
        assert text.strip() == 'removed'
        report['passed'] = True; save()
except BaseException as error:
    report['passed'] = False
    report['failure'] = type(error).__name__ + ': ' + str(error)
    save()
    raise
finally:
    close_viewer()
    client.close()
