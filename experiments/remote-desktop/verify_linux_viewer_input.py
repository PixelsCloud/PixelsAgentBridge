"""GNOME virtual input -> owned fullscreen native viewer -> existing input ABI.

Only generated input is verified; no key content or desktop pixels are saved.
The installed application and system configuration are never changed.
"""
import argparse
import json
import os
import pathlib
import select
import struct
import subprocess
import time


def read(child,size):
    result=bytearray(); deadline=time.monotonic()+8
    while len(result)<size:
        assert time.monotonic()<deadline,'Native input timed out'
        if select.select([child.stdout],[],[],.02)[0]:
            part=os.read(child.stdout.fileno(),size-len(result))
            assert part,'Native input process ended'
            result.extend(part)
    return result


def line(child):
    output=bytearray()
    while not output.endswith(b'\n'):
        assert len(output)<4000
        output.extend(read(child,1))
    return json.loads(output)


def main():
    p=argparse.ArgumentParser(); p.add_argument('probe',type=pathlib.Path); p.add_argument('worker',type=pathlib.Path)
    a=p.parse_args(); probe=a.probe.resolve(strict=True); worker=a.worker.resolve(strict=True)
    assert probe.parent==worker.parent and probe.parent.parent==pathlib.Path('/tmp') and probe.parent.name.startswith('pab-linux-media-')
    screens=json.loads(subprocess.check_output([str(worker),'inventory'],timeout=8))
    assert len(screens)==1 and screens[0]['scale_milli']==1000
    screen=screens[0]; results=[]
    for rotation in range(4):
        view=subprocess.Popen([str(probe),'--input-probe',str(rotation)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
        source=None
        def send(kind,x=0,y=0,value=0,down=False):
            source.stdin.write(struct.pack('<BHHHB',kind,round(x),round(y),value&65535,down))
            assert read(source,1)==b'\1'; time.sleep(.05)
        try:
            ready=line(view); assert ready['ready'] and ready['window']==[screen['pixel_width'],screen['pixel_height']]
            points=[(x/(screen['pixel_width']-1)*65535,y/(screen['pixel_height']-1)*65535) for x,y in ready['points']]
            source=subprocess.Popen([str(worker),'input',screen['id']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
            source.stdin.write(struct.pack('<iiII',screen['x'],screen['y'],screen['desktop_width'],screen['desktop_height']))
            assert read(source,4)==struct.pack('<I',0x31424150)
            x,y=points[0]; send(2,x,y,0,True); send(2,x,y,0,False)
            send(4,value=65,down=True); send(4,value=65,down=False)
            send(4,value=160,down=True); send(4,value=65,down=True); send(4,value=65,down=False); send(4,value=160,down=False)
            send(3,value=-120)
            send(2,x,y,0,True); send(1,*points[1]); send(2,*points[1],0,False)
            send(4,value=162,down=True)
            view.stdin.write(b'v'); result=line(view); assert result['passed']
            assert view.wait(timeout=5)==0
            results.append(result)
        finally:
            if source is not None:
                try:
                    if source.poll() is None: send(5); source.stdin.close(); source.wait(timeout=5)
                finally:
                    if source.poll() is None: source.kill(); source.wait(timeout=5)
            if view.poll() is None: view.kill(); view.wait(timeout=5)
            diagnostics=view.stderr.read().decode(errors='replace')
            if diagnostics: print(json.dumps({'rotation':rotation,'diagnostics':diagnostics}),flush=True)
    print(json.dumps({'passed':True,'suite':'linux_viewer_real_input','cases':results,'workers_reaped':8}),flush=True)


if __name__=='__main__': main()
