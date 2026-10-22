"""Two-hour, three-Relay soak in a new loopback-only fixture; never installs apps."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import socket
import subprocess
import sys
import threading
import time
import tomllib
from urllib.parse import urlsplit

import psutil
import requests
from service_config import write_config

ROOT=Path(__file__).resolve().parents[1]

def save(path,value):
    temporary=path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value,indent=2),encoding='utf-8')
    os.replace(temporary,path)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config',type=Path,required=True)
    parser.add_argument('--binary-dir',type=Path,required=True)
    parser.add_argument('--duration-seconds',type=int,default=7200)
    parser.add_argument('--output',type=Path,help='Fresh evidence directory within this fixture environment')
    args=parser.parse_args()
    assert args.duration_seconds>=7200,'Formal soak cannot shorten the two-hour requirement'
    environment=args.config.resolve();environment.relative_to(ROOT/'.build/admin-console')
    binary_dir=args.binary_dir.resolve();binary_dir.relative_to(ROOT)
    out=args.output.resolve() if args.output else environment.parent/'soak'
    out.relative_to(environment.parent)
    out.mkdir(exist_ok=False)
    fixture_environment=out/'environment.toml'
    shutil.copy2(environment,fixture_environment)
    subprocess.run([sys.executable,str(ROOT/'scripts/serve_admin_console_test.py'),'--config',str(fixture_environment),'--port','38624','--binary-dir',str(binary_dir),'--prepare-only'],cwd=ROOT,check=True)
    # The helper owns a freshly created, randomly named test DB and test TLS key.
    original=out/'https'
    config=tomllib.loads((original/'pab-server.toml').read_text(encoding='utf-8'))
    database=urlsplit(config['database']['url']);assert database.hostname=='127.0.0.1' and database.path.startswith('/admin_browser_')
    env_data=tomllib.loads(environment.read_text(encoding='utf-8'))
    assert config['listen']=='127.0.0.1:38624'
    executable=out/'pab-server.exe';shutil.copy2(original/'pab-server.exe',executable)
    relay_exe=out/'pab-relay-server.exe';shutil.copy2(binary_dir/'pab-relay-server.exe',relay_exe)
    fixture_exe=out/'web-device-fixture.exe';shutil.copy2(binary_dir/'examples/web-device-fixture.exe',fixture_exe)
    reservations=[];nodes=[]
    for index in range(3):
        probe=socket.socket();probe.bind(('127.0.0.1',0));reservations.append(probe)
        nodes.append({'node_id':f'soak-{index}','url':f'https://127.0.0.1:{probe.getsockname()[1]}','region':'isolated','weight':100,'capacity_mbps':100,'control_secret':os.urandom(32).hex()})
    config['relay']['cluster']={'heartbeat_seconds':5,'offline_seconds':15,'recovery_seconds':0,'maintenance_seconds':5,'nodes':nodes}
    config['log']['directory']=str(out/'server-logs')
    server_config=write_config(out/'pab-server.toml',config)
    processes=[];logs=[];device_messages=[];samples=[];errors=[];started=None
    def launch(command,label,stdin=False):
        log=(out/f'{label}.log').open('ab');logs.append(log)
        process=subprocess.Popen(command,cwd=ROOT,stdin=subprocess.PIPE if stdin else subprocess.DEVNULL,stdout=subprocess.PIPE if stdin else log,stderr=log,creationflags=subprocess.CREATE_NO_WINDOW if os.name=='nt' else 0)
        processes.append((label,process));return process
    def request(session,method,path,expected=200,**kwargs):
        response=session.request(method,'https://127.0.0.1:38624'+path,timeout=20,verify=config['tls']['cert'],headers={'Origin':'https://127.0.0.1:38624'},**kwargs)
        assert response.status_code==expected,(method,path,response.status_code)
        return response
    report={'status':'running','duration_required_seconds':args.duration_seconds,'nodes':3,'endpoint_fixtures':2,'period_seconds':30,'connection_scope':'client backend only, excluding observer; background workers recorded separately','thresholds':{'warmup_seconds':300,'rss_growth_mib':128,'private_growth_mib':128,'handle_growth':32,'thread_growth':4,'server_pool_connections':5},'errors':errors}
    try:
        server=launch([str(executable),'serve','--config',str(server_config)],'server')
        time.sleep(1)
        server_monitor=psutil.Process(server.pid)
        # Affinity is bounded for the test process only; host applications are untouched.
        if hasattr(server_monitor,'cpu_affinity'):
            allowed=server_monitor.cpu_affinity();server_monitor.cpu_affinity(allowed[:2]);report['server_cpu_affinity']=allowed[:2]
        admin=requests.Session();ordinary=requests.Session()
        deadline=time.monotonic()+45
        while True:
            try:request(admin,'GET','/health',204);break
            except requests.RequestException:
                assert time.monotonic()<deadline,'fixture failed to start';time.sleep(.2)
        password=os.urandom(24).hex()
        request(admin,'POST','/api/web/register',json={'username':'global-soak-admin','password':password})
        target=request(ordinary,'POST','/api/web/register',json={'username':'global-soak-user','password':password}).json()['id']
        subprocess.run([str(executable),'web-admin','global-soak-admin','--config',str(server_config)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,creationflags=subprocess.CREATE_NO_WINDOW if os.name=='nt' else 0)
        for index,node in enumerate(nodes):
            relay_config=write_config(out/f'relay-{index}.toml',{'node_id':node['node_id'],'cluster_enabled':True,'usage_dir':out/f'usage-{index}','https_bind':f"127.0.0.1:{urlsplit(node['url']).port}",'quic_bind':'127.0.0.1:0','tls':config['tls'],'control':{'url':'wss://127.0.0.1:38624/relay-control','secret':node['control_secret'],'ca_cert':config['tls']['cert']},'log':{'directory':out/f'relay-{index}-logs','level':'info'}})
            reservations[index].close();launch([str(relay_exe),'--config',str(relay_config)],f'relay-{index}')
        for index in range(2):
            process=launch([str(fixture_exe),'wss://127.0.0.1:38624/control',config['tls']['cert'],f'global-soak-device-{index}'],f'endpoint-{index}',True)
            messages=queue.Queue();device_messages.append(messages)
            def drain(source=process.stdout, destination=messages):
                for line in iter(source.readline,b''):
                    try:destination.put(json.loads(line))
                    except ValueError:destination.put({'invalid_fixture_output':True})
            threading.Thread(target=drain,daemon=True).start()
            ready=messages.get(timeout=30);assert ready.get('ready') is True,ready
        deadline=time.monotonic()+60
        while request(admin,'GET','/api/web/relays').json()['summary']['available']!=3:
            assert time.monotonic()<deadline,'three Relay heartbeats did not become healthy';time.sleep(1)
        started=time.monotonic();report['started_at']=datetime.now(timezone.utc).isoformat();report['processes']={label:p.pid for label,p in processes}
        report['binary_sha256']={p.name:hashlib.file_digest(p.open('rb'),'sha256').hexdigest() for p in [executable,relay_exe,fixture_exe]}
        save(out/'baseline.json',report)
        cycle=0
        while True:
            cycle+=1;elapsed=time.monotonic()-started
            assert all(p.poll() is None for _,p in processes),'owned fixture process exited'
            request(admin,'GET','/health',204)
            relay=request(admin,'GET','/api/web/relays').json();assert relay['summary']['available']==3,relay['summary']
            overview=request(admin,'GET','/api/web/overview?scope=all').json();assert overview['online']==2 and overview['total']==2,overview
            for endpoint in ['/api/web/accounts','/api/web/devices?scope=all','/api/web/devices/diagnostics?scope=all','/api/web/usage?interval=lifetime','/api/web/audit?page_size=20']:
                request(admin,'GET',endpoint)
            request(admin,'PUT',f'/api/web/accounts/{target}/traffic',json={'mbps':5 if cycle%2 else 10})
            if cycle%12==0:
                current=request(admin,'GET','/api/web/service').json()['registration']
                request(admin,'PATCH','/api/web/service',json={'revision':current['revision'],'registration_override':False if cycle%24 else None})
            if cycle%10==0:
                for endpoint in ['/api/web/audit/export','/api/web/usage/export?interval=lifetime']:
                    body=request(admin,'GET',endpoint).text;assert 'export_status' in body
            memory=server_monitor.memory_info()
            sample={'elapsed_seconds':round(elapsed,2),'cycle':cycle,'rss_bytes':memory.rss,'private_bytes':getattr(memory,'private',memory.vms),'private_metric':'private' if hasattr(memory,'private') else 'vms','vms_bytes':memory.vms,'threads':server_monitor.num_threads(),'handles':server_monitor.num_handles() if os.name=='nt' else server_monitor.num_fds(),'available_relays':3,'online_devices':2}
            # pg_stat_activity includes autovacuum/parallel workers, which are
            # not SQLx pool connections. Preserve those counts without applying
            # the application's pool limit to PostgreSQL maintenance processes.
            connection_sql="WITH activity AS (SELECT backend_type,state,state_change FROM pg_stat_activity WHERE datname=current_database() AND pid<>pg_backend_pid()) SELECT json_build_object('connections',count(*) FILTER(WHERE backend_type='client backend'),'background_workers',count(*) FILTER(WHERE backend_type<>'client backend'),'backend_types',(SELECT json_object_agg(backend_type,n) FROM (SELECT backend_type,count(*) n FROM activity GROUP BY backend_type) grouped),'stale_idle_transactions',count(*) FILTER(WHERE backend_type='client backend' AND state='idle in transaction' AND state_change<clock_timestamp()-interval '5 seconds')) FROM activity;"
            connections=json.loads(subprocess.check_output(['docker','exec',env_data['container'],'psql','-X','-At','-U','postgres','-d',database.path[1:],'-c',connection_sql],text=True))
            assert connections['connections']<=5 and connections['stale_idle_transactions']==0,connections
            sample.update(connections)
            samples.append(sample)
            with (out/'samples.jsonl').open('a',encoding='utf-8') as sink:sink.write(json.dumps(sample)+'\n')
            save(out/'progress.json',{**report,**sample})
            if elapsed>=args.duration_seconds:break
            time.sleep(min(30,args.duration_seconds-elapsed))
        warmed=[s for s in samples if s['elapsed_seconds']>=300]
        assert warmed
        for key,limit in [('rss_bytes',128*1024*1024),('private_bytes',128*1024*1024),('handles',32),('threads',4)]:
            assert max(s[key] for s in warmed[-20:])-warmed[0][key]<=limit,(key,'post-warmup growth exceeded baseline threshold')
        current=request(admin,'GET','/api/web/service').json()['registration']
        request(admin,'PATCH','/api/web/service',json={'revision':current['revision'],'registration_override':None})
        report.update(status='pass',elapsed_seconds=round(time.monotonic()-started,2),cycles=cycle,finished_at=datetime.now(timezone.utc).isoformat(),resource_first=warmed[0],resource_last=samples[-1])
    except Exception as error:
        errors.append({'class':type(error).__name__,'message':str(error)[:500]})
        report.update(status='fail',elapsed_seconds=round(time.monotonic()-started,2) if started else None)
        raise
    finally:
        for label,process in reversed(processes):
            if process.poll() is None:
                if label.startswith('endpoint'):
                    try:process.stdin.write(b'{"action":"quit"}\n');process.stdin.flush();process.wait(timeout=10)
                    except (BrokenPipeError,subprocess.TimeoutExpired):process.terminate()
                else:process.terminate()
                try:process.wait(timeout=10)
                except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
        for reservation in reservations:reservation.close()
        for log in logs:log.close()
        report['owned_processes_stopped']=all(p.poll() is not None for _,p in processes)
        save(out/'result.json',report)

if __name__=='__main__':main()
