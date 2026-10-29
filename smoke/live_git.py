"""Exercise every exposed Git tool using only new local fixture repositories."""
from __future__ import annotations

import argparse
import base64
import json
import os
from pathlib import Path, PurePosixPath, PureWindowsPath
import time
import tomllib
import uuid

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['lab', 'config', 'mcp', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    lab = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'checks': [], 'calls': [], 'cleanup': []}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')

    env = {key: value for key, value in os.environ.items() if not key.startswith('PAB_')}
    with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as c:
        def call(name, values, allow_error=False):
            entry = {'tool': name, 'arguments': values, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = c.call(name, values, allow_error=allow_error)
                entry['value'] = value
                save()
                value = c.observe(value)
            except McpToolError as error:
                entry['value'] = error.value
                save()
                raise
            entry['value'] = value
            save()
            if not allow_error:
                assert not value.get('error'), name
                assert value.get('result', {}).get('state') not in ['failed', 'cancelled'], name
            return value

        def native(code, family, script):
            if family == 'windows':
                values = {'program': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
                    'args': ['-NoProfile', '-NonInteractive', '-EncodedCommand', base64.b64encode(script.encode('utf-16-le')).decode()]}
            else:
                values = {'program': '/usr/bin/python3', 'args': ['-c', script]}
            value = call('pab_run_command', {'device_code': code, **values, 'wait_ms': 1000, 'timeout_ms': 20000})
            assert (value.get('task') or value.get('snapshot'))['state'] == 'succeeded'
            text = value.get('output', {}).get('stdout', {}).get('text')
            if text is None:
                text = call('pab_read_output', {'device_code': code, 'task_id': value['operation_ref']['task_id'], 'stream': 'stdout'})['text']
            return text.strip()

        def passed(code, name):
            report['checks'].append({'device': code, 'name': name})
            save()
            print(code + ' ' + name + ': passed', flush=True)

        try:
            for device in lab['device']:
                code, family = device['code'], device['os']
                temp = r'C:\Windows\Temp' if family == 'windows' else '/private/tmp' if family == 'macos' else '/tmp'
                root = temp + ('\\pab-git-' if family == 'windows' else '/pab-git-') + str(uuid.uuid4())
                repo, peer, bare = root + '/repo', root + '/peer', root + '/remote.git'
                created = False
                try:
                    v = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    deadline = time.monotonic() + 50
                    while v.get('state') == 'connecting' and time.monotonic() < deadline:
                        v = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    assert v.get('connected')
                    create = ("$ErrorActionPreference='Stop';$r='" + root + "';if(Test-Path -LiteralPath $r){throw 'exists'};[IO.Directory]::CreateDirectory($r)|Out-Null;Write-Output 'created'"
                              if family == 'windows' else "import os;os.mkdir(" + repr(root) + ");print('created')")
                    assert 'created' in native(code, family, create)
                    created = True
                    if family == 'windows':
                        setup = "$ErrorActionPreference='Stop';$r='" + root + "';function g {$p=Start-Process -FilePath ((Get-Command git).Source) -ArgumentList $args -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput ($r+'/git.stdout') -RedirectStandardError ($r+'/git.stderr');if($p.ExitCode -ne 0){throw 'git fixture failed'}};g init --bare --initial-branch=main ($r+'/remote.git');g clone ($r+'/remote.git') ($r+'/repo');"
                        setup += "g -C ($r+'/repo') config user.name PixelsSmoke;g -C ($r+'/repo') config user.email smoke@example.invalid;g -C ($r+'/repo') config commit.gpgsign false;g -C ($r+'/repo') config core.autocrlf false;[IO.File]::WriteAllBytes($r+'/repo/sample.txt',[byte[]](98,97,115,101,10));Write-Output 'created'"
                    else:
                        setup = "import os,subprocess;root=" + repr(root) + ";g=lambda *a:subprocess.run(['git',*a],check=True,capture_output=True);g('init','--bare','--initial-branch=main',root+'/remote.git');g('clone',root+'/remote.git',root+'/repo');g('-C',root+'/repo','config','user.name','Pixels Smoke');g('-C',root+'/repo','config','user.email','smoke@example.invalid');g('-C',root+'/repo','config','commit.gpgsign','false');open(root+'/repo/sample.txt','w').write('base\\n');print('created')"
                    assert 'created' in native(code, family, setup)
                    created = True
                    gitargs = {'device_code': code, 'repo': repo}
                    status = call('pab_git_status', gitargs)
                    assert 'sample.txt' in json.dumps(status)
                    call('pab_git_commit', {**gitargs, 'files': ['sample.txt'], 'message': 'smoke base'})
                    logs = call('pab_git_log', {**gitargs, 'limit': 5})
                    assert 'smoke base' in json.dumps(logs)
                    call('pab_git_push', {**gitargs, 'remote': 'origin', 'branch': 'main'})
                    passed(code, 'status_explicit_commit_log_and_push_to_local_bare_remote')

                    if family == 'windows':
                        prepare_peer = "$ErrorActionPreference='Stop';$r='" + root + "';function g {$p=Start-Process -FilePath ((Get-Command git).Source) -ArgumentList $args -WindowStyle Hidden -Wait -PassThru -RedirectStandardOutput ($r+'/git.stdout') -RedirectStandardError ($r+'/git.stderr');if($p.ExitCode -ne 0){throw 'git fixture failed'}};g clone -b main ($r+'/remote.git') ($r+'/peer');g -C ($r+'/peer') config user.name PixelsSmoke;g -C ($r+'/peer') config user.email smoke@example.invalid;g -C ($r+'/peer') config commit.gpgsign false;[IO.File]::WriteAllText($r+'/peer/peer.txt','peer update');Write-Output 'ready'"
                    else:
                        prepare_peer = "import subprocess;root=" + repr(root) + ";g=lambda *a:subprocess.run(['git',*a],check=True,capture_output=True);g('clone','-b','main',root+'/remote.git',root+'/peer');g('-C',root+'/peer','config','user.name','Pixels Smoke');g('-C',root+'/peer','config','user.email','smoke@example.invalid');g('-C',root+'/peer','config','commit.gpgsign','false');open(root+'/peer/peer.txt','w').write('peer update');print('ready')"
                    assert 'ready' in native(code, family, prepare_peer)
                    call('pab_git_commit', {'device_code': code, 'repo': peer, 'files': ['peer.txt'], 'message': 'smoke peer'})
                    call('pab_git_push', {'device_code': code, 'repo': peer, 'remote': 'origin', 'branch': 'main'})
                    call('pab_git_fetch', {**gitargs, 'remote': 'origin', 'branch': 'main'})
                    call('pab_git_pull', {**gitargs, 'remote': 'origin', 'branch': 'main', 'strategy': 'ff_only'})
                    read = call('pab_file_read', {'device_code': code, 'path': repo + '/peer.txt'})
                    assert 'peer update' in json.dumps(read)
                    passed(code, 'local_peer_push_fetch_ff_pull_content_verified')

                    call('pab_git_checkout', {**gitargs, 'reference': 'HEAD~1', 'detach': True})
                    call('pab_git_checkout', {**gitargs, 'reference': 'main'})
                    call('pab_file_write', {'device_code': code, 'path': repo + '/sample.txt', 'overwrite': True, 'content': 'local unresolved\n'})
                    diff = call('pab_git_diff', {**gitargs, 'paths': ['sample.txt']})
                    assert 'local unresolved' in json.dumps(diff)
                    failed = call('pab_git_checkout', {**gitargs, 'reference': 'missing-smoke-reference'}, allow_error=True)
                    assert failed.get('error') or failed.get('result', {}).get('state') == 'failed'
                    read = call('pab_file_read', {'device_code': code, 'path': repo + '/sample.txt'})
                    assert 'local unresolved' in json.dumps(read)
                    passed(code, 'checkout_detach_restore_diff_and_invalid_ref_preserves_dirty_work')
                finally:
                    if created:
                        cls = PureWindowsPath if family == 'windows' else PurePosixPath
                        target = cls(root)
                        assert target.parent == cls(temp)
                        uuid.UUID(target.name.removeprefix('pab-git-'))
                        result = call('pab_file_delete', {'device_code': code, 'path': root, 'recursive': True, 'max_entries': 1000, 'max_bytes': 10 * 1024 * 1024})
                        report['cleanup'].append({'device': code, 'confirmed': result['result']['state'] == 'completed'})
                    call('pab_disconnect', {'device_code': code})
                    save()
            report['status'] = 'passed'
        except BaseException as error:
            report['status'] = 'failed'
            report['error'] = str(error)
            raise
        finally:
            save()


if __name__ == '__main__':
    main()
