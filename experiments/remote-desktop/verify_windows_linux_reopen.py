import ctypes,json,pathlib,subprocess,sys,time
from ctypes import wintypes as W
import psutil
from windows_owned_surface import capture
import argparse,importlib.util
parser=argparse.ArgumentParser(description="Open/view/close an isolated Windows-to-Linux product viewer 100 times. Requires a staged candidate, generated fullscreen fixture and separately armed rollback.")
parser.add_argument('--profile',type=pathlib.Path,required=True)
parser.add_argument('--remote-stage',required=True)
parser.add_argument('--remote-fixture',required=True)
parser.add_argument('--ssh-helper',type=pathlib.Path,required=True,help="Private credential transport exposing connect() and run(client,password,script,timeout)")
parser.add_argument('--code',required=True)
args=parser.parse_args();assert len(args.code)==9 and args.code.isdecimal()
spec=importlib.util.spec_from_file_location('private_ssh',args.ssh_helper.resolve());linux_ssh=importlib.util.module_from_spec(spec);spec.loader.exec_module(linux_ssh)
base=pathlib.Path(__file__).resolve().parent;root=args.profile.resolve(strict=True)
owner=json.loads((root/'process.json').read_text())['pid']
process=psutil.Process(owner)
assert pathlib.Path(process.exe()).resolve()==(root/'pab-desktop.exe').resolve()
u=ctypes.WinDLL('user32',use_last_error=True)
u.SetProcessDpiAwarenessContext.argtypes=[W.HANDLE];u.SetProcessDpiAwarenessContext(W.HANDLE(-4))
CB=ctypes.WINFUNCTYPE(W.BOOL,W.HWND,W.LPARAM)
u.EnumWindows.argtypes=[CB,W.LPARAM];u.EnumChildWindows.argtypes=[W.HWND,CB,W.LPARAM]
u.GetWindowThreadProcessId.argtypes=[W.HWND,ctypes.POINTER(W.DWORD)]
u.GetWindowTextW.argtypes=[W.HWND,W.LPWSTR,ctypes.c_int];u.GetClassNameW.argtypes=[W.HWND,W.LPWSTR,ctypes.c_int]
u.PostMessageW.argtypes=[W.HWND,W.UINT,W.WPARAM,W.LPARAM]
u.GetClientRect.argtypes=[W.HWND,ctypes.POINTER(W.RECT)];u.ClientToScreen.argtypes=[W.HWND,ctypes.POINTER(W.POINT)]
u.SetWindowPos.argtypes=[W.HWND,W.HWND,ctypes.c_int,ctypes.c_int,ctypes.c_int,ctypes.c_int,W.UINT]
u.GetWindowLongW.argtypes=[W.HWND,ctypes.c_int]
u.WindowFromPoint.argtypes=[W.POINT];u.WindowFromPoint.restype=W.HWND
def windows():
 parents=[];surfaces=[]
 @CB
 def child(h,_):
  cls=ctypes.create_unicode_buffer(128);u.GetClassNameW(h,cls,128)
  if cls.value=='PixelsRemoteDesktopSurface':surfaces.append(h)
  return True
 @CB
 def visit(h,_):
  pid=W.DWORD();u.GetWindowThreadProcessId(h,ctypes.byref(pid))
  if pid.value==owner:
   title=ctypes.create_unicode_buffer(256);u.GetWindowTextW(h,title,256)
   if args.code in title.value.replace(' ',''):parents.append(h);u.EnumChildWindows(h,child,0)
  return True
 u.EnumWindows(visit,0);return parents,surfaces
def ui(action,target):
 r=subprocess.run(['node',str(base/'viewer_reopen_cdp.mjs'),action,args.code],capture_output=True,text=True,timeout=45)
 if r.returncode:raise RuntimeError('UI '+action+' failed: '+r.stderr[-600:])
 return json.loads(r.stdout)
def until(check,seconds=10):
 end=time.monotonic()+seconds
 while True:
  value=check()
  if value:return value
  assert process.is_running() and time.monotonic()<end,'Condition timed out; do not replay mutation'
  time.sleep(.1)
def close():
 parents,_=windows();assert len(parents)==1
 assert u.PostMessageW(parents[0],0x10,0,0)
 until(lambda:not windows()[0] and ui('closed','viewer'))
def pixels():
 parents,surfaces=windows();assert len(parents)==len(surfaces)==1
 parent,surface=parents[0],surfaces[0]
 assert u.SetWindowPos(parent,None,20,40,720,600,0x14)
 time.sleep(.25)
 im=capture(surface,owner)
 rgb=[im.getpixel((round(im.width*fx),round(im.height*fy)))
      for fx,fy in ((.3,.5),(.5,.7),(.7,.5))]
 assert all(max(abs(c-e) for c,e in zip(p,(20,82,74)))<=3 for p in rgb),rgb
 return rgb
def local():
 info=process.memory_info()
 children=[]
 for p in process.children(recursive=True):
  try:
   if pathlib.Path(p.exe()).resolve().parent==root:children.append({'pid':p.pid,'name':p.name()})
  except psutil.NoSuchProcess:pass
 return {'rss':info.rss,'private':info.private,'handles':process.num_handles(),'threads':process.num_threads(),'children':children}
c,password=linux_ssh.connect()
remote_script='import sys\nsys.argv='+repr(['sample',args.remote_stage,args.remote_fixture])+'\n'+(base/'sample_linux_media_processes.py').read_text(encoding='utf-8')
def remote():
 r=json.loads(linux_ssh.run(c,password,remote_script,10))
 assert not r['restored']
 f=r['fixture'];assert f['clicks']==f['releases']==f['ctrl_down']==f['ctrl_up']==0 and not f['ctrl_held'] and not f['left_held']
 return r
def retired():
 r=remote()
 return r if not any(p['worker'] for p in r['processes']) else None
record={'count_requested':100,'cycles':[],'device_code':args.code,
        'pixel_method':'owned native window composed pixels'}
assert not (root/'reopen-cycles.json').exists(),'Preserve the previous run before starting a new one'
def save(): (root/'reopen-cycles.json').write_text(json.dumps(record,indent=2))
try:
 # The initial viewed window was a setup check, not one of the 100 cycles.
 if windows()[0]:close()
 record['baseline_remote']=until(retired);record['baseline_local']=local();save()
 ids=set()
 for i in range(1,101):
  start=time.monotonic();opened=ui('open','main');assert opened.get('accepted'),opened
  until(lambda:len(windows()[0])==1)
  def ready():
   s=ui('status','viewer')
   if s is None:return None
   assert not s.get('error'),s
   return s if s.get('phase')=='viewing' and s.get('frame_ready') else None
  status=until(ready,20);assert status['input_enabled'] is False and status['decoder']=='d3d11_hardware'
  session=status['opened']['config']['session_id'];assert session not in ids;ids.add(session)
  rgb=pixels();view_remote=remote();assert any(p['worker']=='capture' for p in view_remote['processes'])
  close();closed_remote=until(retired);closed_local=local()
  assert not closed_local['children'],'Owned native renderer or worker remained after close'
  record['cycles'].append({'index':i,'elapsed':time.monotonic()-start,'session_id':session,'decoder':status['decoder'],'input_enabled':False,'pixels':rgb,'remote_viewing':view_remote['processes'],'remote_closed':closed_remote['processes'],'local_closed':closed_local})
  save()
  if i%5==0:print(json.dumps({'completed':i,'last_seconds':record['cycles'][-1]['elapsed'],'local_handles':closed_local['handles'],'local_private':closed_local['private']}),flush=True)
 record['passed']=True;save()
except BaseException as error:
 record['passed']=False;record['failure']=type(error).__name__+': '+str(error);save();raise
finally:
 if windows()[0]:close()
 c.close()
