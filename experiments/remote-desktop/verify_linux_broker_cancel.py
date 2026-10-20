"""Opt-in supervised capture cancellation before its first frame request.

Run as the optional script of linux_dma_broker_check in an isolated runtime.
The caller observes broker rejection/errors and subsequent successful capture.
"""
import argparse,json,os,pathlib,select,struct,subprocess,time
p=argparse.ArgumentParser();p.add_argument('worker');p.add_argument('--embedded',action='store_true');p.add_argument('--uinput',action='store_true');a=p.parse_args()
assert os.geteuid()!=0 and os.environ['XDG_SESSION_CLASS']=='user'
command=[a.worker]+(['--media-worker'] if a.embedded else [])
screen=json.loads(subprocess.check_output(command+['inventory'],timeout=5))[0]
results=[]
for i in range(12):
 child=subprocess.Popen(command+['capture-dma',screen['id'],'15','desktop','broker'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
 try:
  data=bytearray();end=time.monotonic()+8
  while len(data)<12:
   assert time.monotonic()<end
   if select.select([child.stdout],[],[],.01)[0]:
    part=os.read(child.stdout.fileno(),12-len(data));assert part,child.stderr.read().decode();data.extend(part)
  assert struct.unpack('<III',data)==(0x31424150,screen['pixel_width'],screen['pixel_height'])
  # Cancel before requesting a frame. No password, lock, or input is involved.
  child.kill();child.wait(timeout=3);results.append({'pid':child.pid,'exit':child.returncode})
 finally:
  if child.poll() is None:child.kill();child.wait(timeout=3)
 time.sleep(.1)
print(json.dumps({'early_capture_cancellations':results,'frames_requested':0}))
