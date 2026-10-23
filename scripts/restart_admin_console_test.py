"""Restart only an explicitly selected isolated browser fixture, preserving its DB."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from urllib.parse import urlsplit
import psutil
from admin_console_fixture_info import fixture_info

ROOT = Path(__file__).resolve().parents[1]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config',type=Path,required=True)
    parser.add_argument('--stop',action='store_true')
    args=parser.parse_args()
    config=args.config.resolve()
    if config.is_relative_to(ROOT / 'smoke/local'):
        sys.path.insert(0, str(ROOT))
        from smoke.web_fixture import restart
        print(json.dumps(restart(config, stop=args.stop)))
        return
    fixture_info(config)
    relative=config.relative_to(ROOT / '.build/admin-console')
    settings=tomllib.loads(config.read_text(encoding='utf-8'))
    database=urlsplit(settings['database']['url'])
    assert database.hostname=='127.0.0.1' and database.path.startswith('/admin_browser_')
    assert settings['listen']=='127.0.0.1:38623' and settings['web']['origin']=='https://127.0.0.1:38623'
    container=json.loads(subprocess.check_output(['docker','inspect','pab-admin-test-20261020'],text=True))[0]
    assert container['Config']['Labels'].get('pixels.task')=='admin-console-20261020'
    assert container['State']['Running'] and any(p['HostIp']=='127.0.0.1' and int(p['HostPort'])==database.port for p in container['NetworkSettings']['Ports']['5432/tcp'])
    executable=config.parent/'pab-server.exe'
    assert executable.is_file() and os.name=='nt'
    owned=[]
    for process in psutil.process_iter(['exe']):
        try:
            if process.info['exe'] and Path(process.info['exe']).resolve()==executable:
                owned.append(process)
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            continue
    for process in owned:
        try:
            process.terminate()
        except psutil.NoSuchProcess:
            pass
    # Termination is asynchronous on Windows. Do not report stopped while the
    # executable is still mapped and prevents a preserving-data replacement.
    for process in owned:
        try:
            process.wait(timeout=15)
        except psutil.NoSuchProcess:
            pass
    if args.stop:
        print(json.dumps({'status':'stopped','fixture':relative.as_posix()})); return
    # No init/migration/config rewrite: reuse the same executable, config and DB.
    with (config.parent/'restart.log').open('ab') as log:
        process=subprocess.Popen([str(executable),'serve','--config',str(config)],cwd=ROOT,stdin=subprocess.DEVNULL,stdout=log,stderr=subprocess.STDOUT,creationflags=subprocess.CREATE_NO_WINDOW)
    print(json.dumps({'status':'started','pid':process.pid,'fixture':relative.as_posix()}))

if __name__=='__main__':main()
