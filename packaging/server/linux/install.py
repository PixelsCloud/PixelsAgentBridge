#!/usr/bin/env python3
"""Versioned native systemd deployment; configuration and data are never replaced."""
import argparse, hashlib, json, os, pathlib, re, shutil, subprocess, time

def digest(path):
    with open(path,'rb') as source: return hashlib.file_digest(source,'sha256').hexdigest()
def run(args,**kwargs):
    return subprocess.run(args,check=True,text=True,capture_output=True,timeout=20,**kwargs).stdout.strip()
def quote(value):
    value=str(value)
    if any(ord(c)<32 for c in value): raise ValueError('Control characters are not allowed in service paths')
    return '"'+value.replace('\\','\\\\').replace('"','\\"').replace('%','%%')+'"'

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--action',choices=['install','update','rollback','restore','remove','status'],default='install')
    parser.add_argument('--component',choices=['server','relay'],required=True)
    parser.add_argument('--root',type=pathlib.Path,required=True)
    parser.add_argument('--source',type=pathlib.Path)
    parser.add_argument('--config',type=pathlib.Path)
    parser.add_argument('--expected-sha256')
    parser.add_argument('--service-name')
    args=parser.parse_args()
    if os.name=='nt' or os.geteuid()!=0: raise ValueError('Native systemd installation requires Linux root')
    root=args.root.resolve()
    if root.parent==root or root in map(pathlib.Path,['/etc','/usr','/var','/opt','/tmp','/home']):
        raise ValueError('Use a dedicated installation directory')
    name=args.service_name or ('pixels-agent-bridge-'+args.component)
    if not re.fullmatch(r'[A-Za-z0-9_.-]{1,80}',name) or name.lower() in ['pixels-agent-bridge-executor','pixelsagentbridgeexecutor']:
        raise ValueError('Invalid or reserved service name')
    if not name.endswith('.service'): name+='.service'
    unit=pathlib.Path('/etc/systemd/system')/name
    state_path=root/(name+'.deployment.json')
    state=json.loads(state_path.read_text()) if state_path.exists() else None
    if unit.exists() and not state: raise ValueError('Existing service is not managed by this installation')
    def owned(path):
        path=pathlib.Path(path).resolve()
        if not path.is_relative_to(root) or path==root: raise ValueError('Versioned path escaped the named installation directory')
        return path
    def unit_for(image):
        exe=owned(image['executable']);config=pathlib.Path(image['config']).resolve()
        command=[str(exe),*(['serve'] if args.component=='server' else []),'--config',str(config)]
        # WorkingDirectory is one path value, not an ExecStart argument list.
        # Quotes become literal characters here and make an absolute path invalid.
        working=str(root).replace('\\','\\\\').replace('%','%%')
        return '[Unit]\nDescription=Pixels '+args.component+' server\nAfter=network-online.target\nWants=network-online.target\n[Service]\nType=simple\nExecStart='+ ' '.join(map(quote,command))+'\nWorkingDirectory='+working+'\nRestart=on-failure\nRestartSec=5\nTimeoutStopSec=10\nKillMode=control-group\nUMask=0077\n[Install]\nWantedBy=multi-user.target\n'
    def activate(image):
        content=unit_for(image)
        if args.component=='server':
            web=owned(pathlib.Path(image['executable']).parent/'web')
            if not (web/'index.html').is_file(): raise ValueError('Versioned Web assets are missing')
            shutil.copytree(web,owned(root/'web'),dirs_exist_ok=True)
        unit.write_text(content)
        run(['systemctl','daemon-reload']);run(['systemctl','enable',name]);run(['systemctl','start',name])
        time.sleep(.5)
        run(['systemctl','is-active',name])
    if args.action=='status':
        active=False
        if root.exists():
            import fcntl
            with (root/(name+'.deploy.lock')).open('a') as status_lock:
                try:fcntl.flock(status_lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
                except BlockingIOError:active=True
        current=subprocess.run(['systemctl','is-active',name],capture_output=True,text=True)
        pid=subprocess.run(['systemctl','show',name,'--property=MainPID','--value'],capture_output=True,text=True).stdout.strip()
        executable=None;sha=None
        if pid.isdigit() and int(pid)>0:
            executable=owned(pathlib.Path('/proc')/pid/'exe')
            sha=digest(executable)
        print(json.dumps({'service':name,'status':current.stdout.strip(),'deployment':state,'installer_active':active,
            'running_binary':str(executable) if executable else None,'running_sha256':sha,
            'release_id':executable.parent.name if executable else None}));return
    root.mkdir(parents=True,exist_ok=True)
    # flock is scoped to this installation; an interrupted process releases it.
    import fcntl
    with (root/(name+'.deploy.lock')).open('a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        if args.action=='remove':
            if unit.exists():
                run(['systemctl','stop',name]);run(['systemctl','disable',name]);unit.unlink();run(['systemctl','daemon-reload'])
            print('Service removed; configuration, data, logs and versioned files retained');return
        if args.action in ['rollback','restore']:
            selected=state.get('previous') if state else None
            if args.action=='restore':
                if not args.expected_sha256 or not re.fullmatch('[a-f0-9]{64}',args.expected_sha256):raise ValueError('Restore requires --expected-sha256')
                selected=next((candidate for candidate in [state.get('current'),state.get('previous')] if candidate and digest(owned(candidate['executable']))==args.expected_sha256),None) if state else None
            if not selected:raise ValueError('Requested recovery version is not recorded')
            unit_for(selected) # Validate before stopping anything.
            if unit.exists():run(['systemctl','stop',name])
            activate(selected)
            if selected!=state['current']:state['current'],state['previous']=selected,state['current']
        else:
            if not args.source or not args.config:raise ValueError('--source and --config are required')
            if args.action=='update' and not state:raise ValueError('Update requires an installed service')
            source=args.source.resolve();config=args.config.resolve(strict=True)
            binary='pab-server' if args.component=='server' else 'pab-relay-server'
            sha=digest(source/binary);release_id=sha
            if args.component=='server':release_id+='-'+digest(source/'web/index.html')[:12]
            release=owned(root/'releases'/release_id);release.mkdir(parents=True,exist_ok=True)
            exe=owned(release/binary)
            if exe.exists():
                if digest(exe)!=sha:raise ValueError('Versioned executable has changed unexpectedly')
            else:shutil.copy2(source/binary,exe);exe.chmod(0o755)
            if args.component=='server':shutil.copytree(source/'web',release/'web',dirs_exist_ok=True)
            with (root/'preflight.log').open('w') as log:
                subprocess.run([str(exe),'check','--config',str(config)],stdout=log,stderr=log,check=True,timeout=30)
            image={'executable':str(exe),'config':str(config),'sha256':sha}
            previous=state['current'] if state else None
            if previous:
                unit_for(previous)
                # Remove intentionally retains state/files. Reinstallation
                # must not fail while stopping a unit that no longer exists.
                if unit.exists():run(['systemctl','stop',name])
            try:activate(image)
            except Exception:
                subprocess.run(['systemctl','stop',name],capture_output=True,timeout=15)
                if previous:activate(previous)
                elif unit.exists():
                    subprocess.run(['systemctl','disable',name],capture_output=True,timeout=15)
                    unit.unlink();run(['systemctl','daemon-reload'])
                raise
            state={'service':name,'component':args.component,'current':image,'previous':previous}
        temporary=state_path.with_suffix('.tmp')
        temporary.write_text(json.dumps(state,indent=2)+'\n');temporary.chmod(0o600);os.replace(temporary,state_path)
        print(json.dumps({'service':name,'deployed':True,'sha256':state['current']['sha256']}))

if __name__=='__main__':main()
