"""Isolated PTY proof over three streams of one authenticated PAB connection."""
import argparse, base64, hashlib, json, os, queue, socket, struct, subprocess, threading, time
from pathlib import Path

p=argparse.ArgumentParser(); p.add_argument('--mcp',required=True); p.add_argument('--device',default='428358431');
p.add_argument('--port',type=int,default=39061); p.add_argument('--mode',choices=['service','user'],default='user');
p.add_argument('--relay',action='store_true'); p.add_argument('--result',required=True); a=p.parse_args()
env=os.environ.copy(); settings=Path(r'C:\Program Files\PixelsAgentBridge\settings.json')
if settings.is_file():
    c=json.loads(settings.read_text(encoding='utf-8-sig')); env.setdefault('PAB_CONTROL_URL',c['control_url']); env.setdefault('PAB_RELAY_URLS',c['relay_urls'])
if a.relay: env['PAB_TEST_RELAY_ONLY']='1'
proc=subprocess.Popen([a.mcp],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,env=env,creationflags=subprocess.CREATE_NO_WINDOW)
pending={}; serial=0; lock=threading.Lock()
def replies():
    for line in proc.stdout:
        v=json.loads(line); q=pending.get(v.get('id'))
        if q: q.put(v)
threading.Thread(target=replies,daemon=True).start()
def rpc(method,params=None,timeout=40):
    global serial
    with lock:
        serial+=1; n=serial; q=queue.Queue(); pending[n]=q
        msg={'jsonrpc':'2.0','id':n,'method':method}
        if params is not None: msg['params']=params
        proc.stdin.write((json.dumps(msg)+'\n').encode()); proc.stdin.flush()
    try:
        v=q.get(timeout=timeout)
        if 'error' in v: raise RuntimeError(v['error'])
        return v['result']
    finally: pending.pop(n,None)
def tool(name,args,timeout=40):
    r=rpc('tools/call',{'name':name,'arguments':args},timeout)
    if r.get('isError'): raise RuntimeError(r.get('content'))
    return r.get('structuredContent') or json.loads(r['content'][0]['text'])
def command(text,**extra):
    return tool('pab_run_command',{'device_code':a.device,'program':'powershell.exe','args':['-NoLogo','-NoProfile','-NonInteractive','-Command',text],'wait_ms':5000,'timeout_ms':15000,**extra})
def exchange(port,v):
    with socket.create_connection(('127.0.0.1',port),timeout=8) as s:
        s.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
        s.sendall((json.dumps(v)+'\n').encode()); f=s.makefile('rb'); return json.loads(f.readline())
def exact(s,n):
    b=bytearray()
    while len(b)<n:
        v=s.recv(n-len(b))
        if not v: raise EOFError()
        b.extend(v)
    return bytes(b)
report={'kind':'independent_pty_over_existing_connection','mode':a.mode,'stages':[]}; forwards=[]; session=None; data=None; admin=None
try:
    rpc('initialize',{'protocolVersion':'2025-03-26','capabilities':{},'clientInfo':{'name':'desktop-terminal-proof','version':'1'}})
    with lock: proc.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n'); proc.stdin.flush()
    tool('pab_connect',{'device_code':a.device})
    inventory=tool('pab_list_execution_contexts',{'device_code':a.device,'user':'Administrator'})
    selection=next(x['selection'] for x in inventory['result']['data']['entries'] if x['mode']==a.mode)
    root=r'C:\Users\Administrator\Downloads\PixelsTerminalProof-20261014'
    launch=tool('pab_run_command',{'device_code':a.device,'program':root+r'\terminal-proof-server.exe','args':[str(a.port),root+'\\'+str(a.port)],'execution':selection,'wait_ms':1000,'timeout_ms':300000})
    report['launch_task']=launch['operation_ref']['task_id']
    for attempt in range(15):
        ready=command(f"Get-Content -LiteralPath '{root}\\{a.port}\\ready.json' -Raw")
        if ready['task']['state']=='succeeded': report['ready']=json.loads(ready['output']['stdout']['text']); break
        time.sleep(.3)
    else: raise RuntimeError('server not ready')
    for port in range(a.port,a.port+3):
        f=tool('pab_port_forward_open',{'device_code':a.device,'remote_port':port}); forwards.append(f)
    data_port,ctrl,admin=[f['local_port'] for f in forwards]
    report['paths']=[f['connection_path'] for f in forwards]
    data=socket.create_connection(('127.0.0.1',data_port),timeout=10); data.settimeout(120)
    data.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
    parsed=bytearray(); digest=hashlib.sha256(); total=0; pause=threading.Event(); failure=[]
    def receive():
        global total
        try:
            while True:
                n=struct.unpack('<I',exact(data,4))[0]
                if not n: break
                b=exact(data,n); digest.update(b); total+=n; parsed.extend(b)
                if len(parsed)>262144: del parsed[:-262144]
                if b'\x1b[6n' in b: exchange(ctrl,{'action':'input','data':base64.b64encode(b'\x1b[1;1R').decode()})
                while pause.is_set(): time.sleep(.01)
                data.sendall(struct.pack('<Q',total))
        except Exception as e: failure.append(str(e))
    receiver=threading.Thread(target=receive,daemon=True); receiver.start()
    def send(text):
        v=exchange(ctrl,{'action':'input','data':base64.b64encode(text.encode()).decode()}); assert v['result']=='applied',v
    def wait(marker,timeout=20):
        until=time.monotonic()+timeout; needle=marker.encode()
        while time.monotonic()<until:
            if needle in parsed: return
            time.sleep(.03)
        raise TimeoutError((marker,bytes(parsed[-1000:]),failure))
    print('PTY attached',flush=True);time.sleep(1)
    send("$env:PAB_TERM_PROOF='kept'; Set-Location $env:USERPROFILE; Write-Output ('__'+'READY__')\r")
    wait('__READY__'); parsed.clear()
    send("Write-Output ('STATE:'+$env:PAB_TERM_PROOF); Write-Output ('HOME:'+(Get-Location).Path); whoami; Write-Output ('__'+'IDENTITY__')\r")
    wait('__IDENTITY__'); assert b'STATE:kept' in parsed,bytes(parsed[-2000:]); report['stages'].append({'identity':bytes(parsed[-2000:]).decode(errors='replace')})
    session=tool('pab_open_terminal',{'device_code':a.device})
    old_id=session.get('session_id') or session.get('sessionId')
    if not old_id: raise RuntimeError(session)
    report['stable_terminal_id']=old_id
    pause.set(); parsed.clear()
    send("$block=('X'*79+[Environment]::NewLine)*200; for($i=0;$i -lt 10000;$i++){[Console]::Write($block)}; Write-Output ('__'+'FLOOD_DONE__')\r")
    time.sleep(2); t0=time.monotonic(); stats=exchange(admin,{'action':'stats'}); report['pause_stats']=stats
    assert stats['max_inflight']<=262144 and stats['produced']-stats['acked']<=262144,stats
    stable=command("Write-Output 'stable_command_while_paused'"); assert stable['task']['state']=='succeeded',stable
    tool('pab_terminal_input',{'session_id':old_id,'data':"Write-Output ('__'+'STABLE_WHILE_PAUSED__')\r"})
    stable_output=''
    for _ in range(25):
        part=tool('pab_terminal_read',{'session_id':old_id}); stable_output+=part.get('data',part.get('text',''))
        if '__STABLE_WHILE_PAUSED__' in stable_output: break
        time.sleep(.04)
    assert '__STABLE_WHILE_PAUSED__' in stable_output,(part,stable_output)
    report['stable_terminal_while_paused']=True
    print('Paused output: stable command and terminal OK',flush=True)
    resize=exchange(ctrl,{'action':'resize'}); assert resize['result']=='applied',resize
    report['stable_command_ms']=(time.monotonic()-t0)*1000
    t0=time.monotonic(); send('\x03'); report['interrupt_ack_ms']=(time.monotonic()-t0)*1000
    pause.clear(); time.sleep(.7); parsed.clear()
    send("Write-Output ('STATE:'+$env:PAB_TERM_PROOF); Write-Output ('__'+'AFTER_INTERRUPT__')\r"); wait('__AFTER_INTERRUPT__')
    assert b'STATE:kept' in parsed,bytes(parsed[-1000:]); report['stages'].append({'interrupt_recovered':True})
    print('Ctrl+C recovered same Shell',flush=True)
    parsed.clear(); baseline=total
    send("$block=('Y'*79+[Environment]::NewLine)*200; for($i=0;$i -lt 1100;$i++){[Console]::Write($block)}; Write-Output ('__'+'LARGE_DONE__')\r")
    wait('__LARGE_DONE__',200); assert total-baseline>16*1024*1024,total-baseline
    report['large_output_bytes']=total-baseline
    time.sleep(.2); stats=exchange(admin,{'action':'stats'}); assert stats['sha256']==digest.hexdigest(),(stats,total)
    report['output_sha256']=digest.hexdigest(); report['total_output_bytes']=total
    print('Large output exact hash OK',total,flush=True)
    parsed.clear();send("Write-Output ('__'+'HANG__'); Start-Sleep -Seconds 999\r");wait('__HANG__')
    t0=time.monotonic(); stats=exchange(admin,{'action':'kill'}); report['kill_ms']=(time.monotonic()-t0)*1000; assert stats['exit'] is not None,stats
    receiver.join(1); report['reader_eof_after_shell_kill']=not receiver.is_alive()
    report['final_stats']=exchange(admin,{'action':'shutdown'}); admin=None
    receiver.join(5); assert not receiver.is_alive(),'reader survived dedicated worker shutdown'
    report['reader_eof_after_worker_shutdown']=True
    tool('pab_terminal_close',{'session_id':old_id}); session=None
    report['success']=True
except Exception as error:
    report['success']=False; report['error']=str(error); report['received_bytes_at_failure']=globals().get('total',0)
    report['forward_status_at_failure']=[tool('pab_port_forward_status',{'forward_id':f['forward_id']}) for f in forwards]
    raise
finally:
    if data:
        try: data.close()
        except OSError: pass
    if admin:
        try: exchange(admin,{'action':'shutdown'})
        except Exception: pass
    if session:
        try: tool('pab_terminal_close',{'session_id':session.get('session_id') or session.get('sessionId')})
        except Exception: pass
    for f in forwards:
        try: tool('pab_port_forward_close',{'forward_id':f['forward_id']})
        except Exception: pass
    proc.stdin.close()
    try:proc.wait(timeout=8)
    except subprocess.TimeoutExpired:proc.kill()
    Path(a.result).parent.mkdir(parents=True,exist_ok=True);Path(a.result).write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(report,ensure_ascii=False))
