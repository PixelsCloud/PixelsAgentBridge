"""Exercise native service controls using only uniquely named disposable services.

Requires administrator/root access on explicitly configured Windows/macOS test
hosts. Never controls the Executor or any existing service. The Windows fixture
is built separately; this runner neither compiles nor updates installed apps.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib
import uuid

from mcp_client import McpClient, McpToolError


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('lab', 'config', 'mcp', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--windows-fixture', type=Path)
    args = parser.parse_args()
    devices = tomllib.loads(args.lab.read_text(encoding='utf-8-sig'))['device']
    assert devices and all(d['os'] in ('windows', 'macos') for d in devices)
    if any(d['os'] == 'windows' for d in devices):
        assert args.windows_fixture and args.windows_fixture.is_file(), 'Build the Windows service fixture first'
    args.output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'running', 'checks': [], 'calls': [], 'fixtures': [], 'cleanup': [],
              'mcp_sha256': hashlib.sha256(args.mcp.read_bytes()).hexdigest()}

    def save():
        (args.output / 'result.json').write_text(json.dumps(report, indent=2), encoding='utf-8')

    env = {k: v for k, v in os.environ.items() if not k.startswith('PAB_')}
    with McpClient(args.mcp.resolve(), args.output / 'mcp.log', config=args.config.resolve(), env=env) as client:
        def call(tool, values, *, allow_error=False):
            entry = {'tool': tool, 'arguments': values, 'value': None}
            report['calls'].append(entry)
            save()
            try:
                value = client.call(tool, values, allow_error=allow_error)
                entry['value'] = value
                save()
                value = client.observe(value)
            except McpToolError as error:
                entry['value'] = error.value
                save()
                raise
            entry['value'] = value
            save()
            if not allow_error:
                assert not value.get('error'), tool
                assert value.get('result', {}).get('state') not in ('failed', 'cancelled'), tool
            return value

        def native(code, family, script):
            command = ({'program': r'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
                        'args': ['-NoProfile', '-NonInteractive', '-EncodedCommand',
                                 base64.b64encode(script.encode('utf-16-le')).decode()]}
                       if family == 'windows' else {'program': '/usr/bin/python3', 'args': ['-c', script]})
            value = call('pab_run_command', {'device_code': code, **command, 'wait_ms': 1000, 'timeout_ms': 25000})
            assert (value.get('task') or value.get('snapshot'))['state'] == 'succeeded', 'fixture command'
            text = value.get('output', {}).get('stdout', {}).get('text')
            if text is None:
                text = call('pab_read_output', {'device_code': code,
                    'task_id': value['operation_ref']['task_id'], 'stream': 'stdout'})['text']
            return text.strip()

        def checked(code, name):
            report['checks'].append({'device': code, 'name': name, 'passed': True})
            save()
            print(code + ' ' + name + ': passed', flush=True)

        try:
            for device in devices:
                code, family = device['code'], device['os']
                token = str(uuid.uuid4())
                name = 'PabSmoke-' + token if family == 'windows' else 'system:com.pab.smoke.' + token
                root = r'C:\Windows\Temp\pab-service-' + token
                executable = root + r'\fixture.exe'
                label = 'com.pab.smoke.' + token
                plist = '/Library/LaunchDaemons/' + label + '.plist'
                fixture = {'device': code, 'os': family, 'name': name, 'path': root if family == 'windows' else plist}
                report['fixtures'].append(fixture)
                save()  # Persist intent before creating resources, including uncertain failures.
                created = False
                try:
                    value = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    deadline = time.monotonic() + 50
                    while value.get('state') == 'connecting' and time.monotonic() < deadline:
                        value = call('pab_connect', {'device_code': code, 'wait_ms': 1000})
                    assert value.get('connected')
                    if family == 'windows':
                        assert 'created' in native(code, family, "$ErrorActionPreference='Stop';$r='" + root +
                            "';if(Test-Path -LiteralPath $r){throw 'exists'};[IO.Directory]::CreateDirectory($r)|Out-Null;'created'")
                        created = True
                        call('pab_upload_file', {'device_code': code, 'source': str(args.windows_fixture.resolve()),
                            'destination': executable, 'overwrite': False, 'wait_ms': 1000})
                        expected = hashlib.sha256(args.windows_fixture.read_bytes()).hexdigest()
                        setup = "$ErrorActionPreference='Stop';$exe='" + executable + "';$n='" + name + "';"
                        setup += "if((Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLower() -ne '" + expected + "'){throw 'fixture hash'};"
                        setup += "if(Get-Service -Name $n -ErrorAction SilentlyContinue){throw 'service exists'};"
                        setup += "New-Service -Name $n -BinaryPathName ('\"'+$exe+'\" '+$n) -StartupType Manual|Out-Null;'ready'"
                    else:
                        # Exclusive creation and a validated unique label prevent replacing any installed job.
                        setup = ("import os,plistlib;path=" + repr(plist) + ";label=" + repr(label) +
                            ";data={'Label':label,'Program':'/bin/sleep','ProgramArguments':['/bin/sleep','120'],"
                            "'RunAtLoad':False,'KeepAlive':False};f=open(path,'xb');"
                            "plistlib.dump(data,f);f.close();os.chmod(path,0o644);print('ready')")
                        created = True
                    assert 'ready' in native(code, family, setup)

                    def info():
                        return call('pab_get_service', {'device_code': code, 'name': name})['result']['data']['service']

                    def control(action, timeout=10000, allow_error=False):
                        value = call('pab_service_control', {'device_code': code, 'name': name,
                            'control': action, 'timeout_ms': timeout}, allow_error=allow_error)
                        result = value.get('result', {}).get('data', {}).get('result', {})
                        if not allow_error:
                            assert result.get('outcome') == 'completed', action + ' not confirmed'
                        return result

                    stopped = 'stopped' if family == 'windows' else 'not_loaded'
                    assert info()['state'] == stopped
                    control('enable')
                    assert info()['start_mode'].lower() in ('autostart', 'auto_start', 'automatic', 'auto', 'enabled'), 'enabled start mode'
                    control('disable')
                    assert info()['start_mode'].lower() == 'disabled'
                    control('enable')
                    checked(code, 'enable_disable_observed_in_native_configuration')

                    control('start')
                    before = info()
                    assert before['state'].lower() == 'running' and before['pid']
                    unchanged = control('start')
                    assert unchanged['changed'] is False and info()['pid'] == before['pid']
                    inventory = call('pab_list_services', {'device_code': code, 'name': name})
                    assert name in json.dumps(inventory)
                    checked(code, 'start_and_repeated_start_preserve_running_pid')

                    control('restart')
                    after = info()
                    assert after['state'].lower() == 'running' and after['pid'] != before['pid']
                    control('stop')
                    assert info()['state'].lower() == stopped
                    unchanged = control('stop')
                    assert unchanged['changed'] is False
                    control('start')
                    assert info()['pid']
                    control('stop')
                    checked(code, 'restart_pid_changes_stop_idempotent_and_start_after_stop')
                finally:
                    if created:
                        if family == 'windows':
                            cleanup = "$ErrorActionPreference='Stop';$n='" + name + "';$exe='" + executable + "';"
                            cleanup += "$s=Get-CimInstance Win32_Service -Filter (\"Name='\"+$n+\"'\");if($s){if($s.PathName -ne ('\"'+$exe+'\" '+$n)){throw 'service ownership'};"
                            cleanup += "Stop-Service -Name $n -ErrorAction Stop;$s.Delete()|Out-Null};'removed'"
                            # Invoke-CimMethod works with Get-CimInstance on all supported PS versions.
                            cleanup = cleanup.replace('$s.Delete()|Out-Null', "if((Invoke-CimMethod -InputObject $s -MethodName Delete).ReturnValue -ne 0){throw 'delete failed'}")
                            assert 'removed' in native(code, family, cleanup)
                            deleted = call('pab_file_delete', {'device_code': code, 'path': root,
                                'recursive': True, 'max_entries': 4, 'max_bytes': 1024 * 1024, 'max_depth': 1})
                            assert deleted['result']['state'] == 'completed'
                            verified = native(code, family, "if((Get-Service -Name '" + name + "' -ErrorAction SilentlyContinue) -or (Test-Path -LiteralPath '" + root + "')){throw 'cleanup incomplete'};'clean'")
                        else:
                            cleanup = ("import os,plistlib,subprocess;path=" + repr(plist) + ";label=" + repr(label) +
                                ";target='system/'+label;"
                                "r=subprocess.run(['/bin/launchctl','bootout',target],capture_output=True);"
                                "r=subprocess.run(['/bin/launchctl','print',target],capture_output=True);"
                                "assert r.returncode != 0 and b'Could not find service' in r.stderr;"
                                "assert not os.path.islink(path);f=open(path,'rb');p=plistlib.load(f);f.close();"
                                "assert p['Label']==label and p['Program']=='/bin/sleep';os.unlink(path);print('clean')")
                            verified = native(code, family, cleanup)
                        assert 'clean' in verified
                        report['cleanup'].append({'device': code, 'name': name, 'confirmed': True})
                        save()
                    call('pab_disconnect', {'device_code': code})
            report['status'] = 'passed'
        except BaseException as error:
            report['status'] = 'failed'
            report['error'] = repr(error)
            raise
        finally:
            save()


if __name__ == '__main__':
    main()
