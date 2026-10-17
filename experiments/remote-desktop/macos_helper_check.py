"""Opt-in Apple Aqua helper acceptance; run as the active graphical user.

Requires a signed candidate App, compiled executor test binary and the compiled
macos_input_fixture.m under an isolated /private/tmp/pab-* directory. The App
must already have Screen Recording and Accessibility grants, and any additional
system direct-capture consent must be completed. Do not repeat a failed run while
a system consent dialog remains open. Uses a temporary
LaunchAgent and private IPC token; never replaces installed services or TCC.
"""
import argparse,hashlib,json,os,pathlib,plistlib,re,shutil,socket,subprocess,time,uuid
parser=argparse.ArgumentParser(description=__doc__)
for name in ['stage','app','test','fixture']:
 parser.add_argument('--'+name,type=pathlib.Path,required=True)
args=parser.parse_args();stage=args.stage.resolve();uid=os.getuid()
assert stage.parent==pathlib.Path('/private/tmp') and stage.name.startswith('pab-')
assert uid>0 and subprocess.check_output(['/usr/bin/stat','-f','%u','/dev/console'],text=True).strip()==str(uid)
app_bundle=args.app.resolve();app=app_bundle/'Contents/MacOS/pab-desktop'
test=args.test.resolve();fixture_binary=args.fixture.resolve()
for path in [app,test,fixture_binary]:assert path.is_file() and path.is_relative_to(stage)
subprocess.run(['/usr/bin/codesign','--verify','--strict',str(app_bundle)],check=True,capture_output=True)
build={'sha256':hashlib.sha256(test.read_bytes()).hexdigest()}
def processes(executable):
 found=[]
 for line in subprocess.check_output(['/bin/ps','-axo','pid=,command='],text=True).splitlines():
  parts=line.strip().split(None,1)
  if len(parts)==2 and (parts[1]==str(executable) or parts[1].startswith(str(executable)+' ')):found.append(int(parts[0]))
 return found
assert not processes(app),'Candidate App already in use'
root=stage/('pab-helper-ipc-'+uuid.uuid4().hex[:12]);root.mkdir(mode=0o700)
with socket.socket() as sock:sock.bind(('127.0.0.1',0));port=sock.getsockname()[1]
label='com.pixelsagentbridge.test-media-'+root.name.removeprefix('pab-helper-ipc-')
domain='gui/'+str(uid);service=domain+'/'+label
env=os.environ.copy();env.update(PAB_DATA_DIR=str(root),PAB_LOCAL_IPC_PORT=str(port))
plist=root/'helper.plist'
plist.write_bytes(plistlib.dumps({'Label':label,'ProgramArguments':[str(app),'--session-helper'],'LimitLoadToSessionType':'Aqua','RunAtLoad':True,'EnvironmentVariables':{'PAB_DATA_DIR':str(root),'PAB_LOCAL_IPC_PORT':str(port)},'StandardOutPath':str(root/'helper.out'),'StandardErrorPath':str(root/'helper.err')}))
report={'root':str(root),'port':port,'signed_app_sha256':hashlib.sha256(app.read_bytes()).hexdigest(),'test_sha256':build['sha256'],'helper_started':False}
def alive(pid):
 if not pid:return False
 try:os.kill(pid,0);return True
 except ProcessLookupError:return False
def wait_file(name,timeout):
 deadline=time.monotonic()+timeout
 while not (root/name).exists():
  assert server.poll() is None,'Server exited before '+name
  assert time.monotonic()<deadline,'Timeout waiting for '+name
  time.sleep(.1)
def stopped(timeout):
 deadline=time.monotonic()+timeout
 while alive(report.get('helper_pid')):
  assert time.monotonic()<deadline,'Helper failed to exit'
  time.sleep(.1)
server=None;loaded=False;fixture=None
try:
 bundle=root/'InputFixture.app';binary=bundle/'Contents/MacOS/input-fixture';binary.parent.mkdir(parents=True)
 shutil.copy2(fixture_binary,binary)
 (bundle/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier':'vip.rgaa.pab.input-fixture','CFBundleExecutable':'input-fixture','CFBundleName':'Pixels input acceptance','CFBundlePackageType':'APPL'}))
 with (root/'fixture.log').open('wb') as log:
  fixture=subprocess.Popen(['/usr/bin/open','-n','-W','--stderr',str(root/'fixture-events.log'),str(bundle),'--args',str(root/'input-fixture.json')],stdout=log,stderr=subprocess.STDOUT)
 for _ in range(100):
  assert fixture.poll() is None,'Fixture exited'
  file=root/'input-fixture.json'
  if file.exists() and json.loads(file.read_text()).get('focused'):break
  time.sleep(.05)
 else:raise RuntimeError('Owned input fixture did not acquire focus')
 report['fixture_pid']=json.loads(file.read_text())['pid']
 with (root/'server.log').open('wb') as log:
  server=subprocess.Popen([str(test),'local_ipc::media::tests::macos_launchagent_native_media_reopens_and_retires','--exact','--ignored','--nocapture'],env=env,stdout=log,stderr=subprocess.STDOUT)
  wait_file('server-ready',15)
  subprocess.run(['/bin/launchctl','bootstrap',domain,str(plist)],check=True,capture_output=True);loaded=True
  state=subprocess.check_output(['/bin/launchctl','print',service],text=True)
  for _ in range(30):
   match=re.search(r'\bpid = (\d+)',state)
   if match:break
   time.sleep(.1);state=subprocess.check_output(['/bin/launchctl','print',service],text=True)
  assert match,'No launched helper PID';report.update(helper_pid=int(match[1]),helper_started=True)
  print(json.dumps(report),flush=True)
  wait_file('cycles-complete',100)
  subprocess.run(['/bin/launchctl','bootout',service],check=True,capture_output=True);loaded=False
  stopped(15);(root/'helper-stopped').write_text('stopped')
  report['test_exit_code']=server.wait(timeout=35)
  assert report['test_exit_code']==0,'Native helper test failed'
  events=(root/'fixture-events.log').read_text()
  assert 'bundle=vip.rgaa.pab.desktop ' not in events,'Media worker stole focus'
  report['media_worker_stole_focus']=False
  report['passed']=True
except Exception as e:
 report['error']=str(e);report['passed']=False
finally:
 if loaded:
  result=subprocess.run(['/bin/launchctl','bootout',service],capture_output=True);report['cleanup_bootout']=result.returncode
 try:stopped(15)
 except Exception as e:report['cleanup_error']=str(e)
 if server and server.poll() is None:
  server.terminate();server.wait(timeout=10)
 if fixture:
  file=root/'input-fixture.json'
  if file.exists():
   report['fixture_result']=json.loads(file.read_text())
  for pid in processes(binary):os.kill(pid,15)
  fixture.wait(timeout=10)
 report['fixture_running_after']=bool(processes(binary)) if fixture else False
 report['helper_running_after']=alive(report.get('helper_pid'))
 report['candidate_processes_remaining']=len(processes(app))
 if report['helper_running_after'] or report['fixture_running_after'] or report['candidate_processes_remaining']:
  report['passed']=False;report['cleanup_error']='Owned processes remain'
 report['server_exit_code']=server.poll() if server else None
 for name in ['server.log','helper.err','fixture.log','fixture-events.log']:
  file=root/name
  report[name]=file.read_text(errors='replace')[-5000:] if file.exists() else ''
 (root/'result.json').write_text(json.dumps(report,indent=2))
 print(json.dumps(report),flush=True)
if not report.get('passed'):raise SystemExit(1)
