#!/usr/bin/env python3
"""Manage only one existing Compose Relay service; never replaces volumes/config."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def run(args, timeout=60):
    # Pull/build diagnostics can be large and may contain private registry names.
    with tempfile.TemporaryFile() as output:
        result=subprocess.run(args,stdout=output,stderr=output,timeout=timeout)
        if result.returncode:raise RuntimeError('Docker operation failed; inspect target Docker/service logs')
        if output.tell()>2*1024*1024:raise RuntimeError('Docker response is too large')
        output.seek(0);return output.read().decode().strip()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--action',choices=['stage','status','update','restore'],required=True)
    parser.add_argument('--root',type=Path,required=True)
    parser.add_argument('--compose-file',type=Path,required=True)
    parser.add_argument('--project-directory',type=Path,required=True)
    parser.add_argument('--project-name',required=True)
    parser.add_argument('--service-name',required=True)
    parser.add_argument('--image')
    parser.add_argument('--expected-sha256')
    args=parser.parse_args()
    for value in [args.project_name,args.service_name]:
        if not re.fullmatch('[A-Za-z0-9_.-]{1,80}',value):raise ValueError('Invalid Compose project/service name')
    if args.service_name.lower() in ['postgres','database','backend','server','executor','pixelsagentbridgeexecutor']:
        raise ValueError('Only a dedicated Relay service may be upgraded')
    root=args.root.resolve()
    if root.parent==root:raise ValueError('Use a dedicated staging directory')
    project=args.project_directory.resolve(strict=True);compose=args.compose_file.resolve(strict=True)
    command=['docker','compose','--project-directory',str(project),'-f',str(compose),'-p',args.project_name]
    root.mkdir(parents=True,exist_ok=True)
    state_path=root/'deployment.json';override=root/'compose.override.json'
    identity=hashlib.sha256(json.dumps([str(compose),str(project),args.project_name,args.service_name]).encode()).hexdigest()
    state=json.loads(state_path.read_text()) if state_path.exists() else None
    if state and state['identity']!=identity:raise ValueError('Staging directory belongs to another Compose service')
    def current():
        ids=run(command+['ps','--all','--quiet',args.service_name]).splitlines()
        if len(ids)!=1:raise RuntimeError('Exactly one existing Relay container is required')
        image,running=run(['docker','inspect','--format','{{.Image}} {{.State.Running}}',ids[0]]).split()
        if not re.fullmatch('sha256:[a-f0-9]{64}',image):raise ValueError('Unexpected Docker image identity')
        return image[7:],running=='true'
    def write_state():
        temporary=state_path.with_suffix('.tmp');temporary.write_text(json.dumps(state,indent=2)+'\n');temporary.chmod(0o600);os.replace(temporary,state_path)
    def activate(image):
        if not re.fullmatch('[a-f0-9]{64}',image):raise ValueError('Invalid recovery image')
        # This is a third-party Compose runtime contract, not owned app configuration.
        override.write_text(json.dumps({'services':{args.service_name:{'image':'sha256:'+image,'pull_policy':'never'}}}))
        override.chmod(0o600)
        run(command+['-f',str(override),'config','--quiet'])
        run(command+['-f',str(override),'up','-d','--no-deps','--force-recreate',args.service_name],timeout=90)
    lock_path=root/'deploy.lock'
    with lock_path.open('a') as lock:
        busy=False
        try:fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        except BlockingIOError:
            if args.action!='status':raise RuntimeError('Another deployment is active')
            busy=True
        if args.action=='status':
            image,running=current()
            print(json.dumps({'installer_active':busy,'running_sha256':image if running else None,
                'status':'Running' if running else 'Stopped','release_id':image,'deployment':state}));return
        if args.action=='stage':
            if not args.image or not re.fullmatch(r'[^\s@]+@sha256:[a-f0-9]{64}',args.image):raise ValueError('Pin the registry image by SHA-256 digest')
            run(command+['config','--quiet'])
            image,running=current()
            if not running:raise RuntimeError('Existing Relay container must be running before staging')
            if state:
                if state['image']!=args.image:raise ValueError('Staging directory belongs to another version')
                print(json.dumps({'sha256':state['target']}));return
            run(['docker','pull',args.image],timeout=300)
            target=run(['docker','image','inspect','--format','{{.Id}}',args.image])
            target=target[7:] if target.startswith('sha256:') else target
            if not re.fullmatch('[a-f0-9]{64}',target):raise ValueError('Unexpected pulled image identity')
            state={'identity':identity,'image':args.image,'target':target,'previous':image}
            write_state();print(json.dumps({'sha256':target}));return
        if not state:raise ValueError('Stage must complete before changing the container')
        image,running=current()
        if image not in [state['previous'],state['target']]:raise ValueError('Another operator changed the running container')
        selected=state['target']
        if args.action=='restore':
            if args.expected_sha256!=state['previous']:raise ValueError('Recovery digest differs from the recorded previous image')
            selected=state['previous']
        if image!=selected or not running:activate(selected)
        image,running=current()
        if image!=selected or not running:raise RuntimeError('Container did not start the selected image')
        print(json.dumps({'deployed':True,'sha256':image}))


if __name__=='__main__':main()
