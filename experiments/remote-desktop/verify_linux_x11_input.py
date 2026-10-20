"""Real XTEST events on the two Xvfb screens owned by verify_linux_x11.py.

Only generated Ctrl/A/mouse input is used. The same production worker and
recover-input command used by NativeInput run here; no desktop user is targeted.
"""
import ctypes as C
import json
import os
import pathlib
import select
import struct
import subprocess
import time


class Event(C.Structure):
    _fields_ = [('type',C.c_int),('serial',C.c_ulong),('send_event',C.c_int),
        ('display',C.c_void_p),('window',C.c_ulong),('root',C.c_ulong),
        ('subwindow',C.c_ulong),('time',C.c_ulong),('x',C.c_int),('y',C.c_int),
        ('x_root',C.c_int),('y_root',C.c_int),('state',C.c_uint),
        # XNextEvent writes an XEvent union (24 native longs), not just the
        # smaller XButtonEvent prefix. Reserve at least that much trailing room.
        ('detail',C.c_uint),('same_screen',C.c_int),('padding',C.c_long*24)]


def check_input(worker, env, screens):
    x=C.CDLL('libX11.so.6')
    def fn(name,result,*args):
        f=getattr(x,name);f.restype=result;f.argtypes=args;return f
    fn('XOpenDisplay',C.c_void_p,C.c_char_p)
    fn('XCloseDisplay',C.c_int,C.c_void_p)
    fn('XRootWindow',C.c_ulong,C.c_void_p,C.c_int)
    fn('XCreateSimpleWindow',C.c_ulong,C.c_void_p,C.c_ulong,C.c_int,C.c_int,C.c_uint,C.c_uint,C.c_uint,C.c_ulong,C.c_ulong)
    fn('XMapRaised',C.c_int,C.c_void_p,C.c_ulong)
    fn('XDestroyWindow',C.c_int,C.c_void_p,C.c_ulong)
    fn('XSetInputFocus',C.c_int,C.c_void_p,C.c_ulong,C.c_int,C.c_ulong)
    fn('XSelectInput',C.c_int,C.c_void_p,C.c_ulong,C.c_long)
    fn('XSync',C.c_int,C.c_void_p,C.c_int)
    fn('XPending',C.c_int,C.c_void_p)
    fn('XNextEvent',C.c_int,C.c_void_p,C.POINTER(Event))
    fn('XQueryKeymap',C.c_int,C.c_void_p,C.c_void_p)
    fn('XQueryPointer',C.c_int,C.c_void_p,C.c_ulong,C.POINTER(C.c_ulong),C.POINTER(C.c_ulong),
       C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_uint))
    connection=x.XOpenDisplay(env['DISPLAY'].encode());assert connection
    children=[];windows=[];results=[]
    def events():
        x.XSync(connection,False);result=[]
        while x.XPending(connection):
            e=Event();x.XNextEvent(connection,C.byref(e))
            if e.type in (2,3,4,5):result.append([e.type,e.detail,e.x_root,e.y_root])
        return result
    def key_down(code):
        keys=(C.c_ubyte*32)();x.XQueryKeymap(connection,keys)
        return bool(keys[code//8]&(1<<(code%8)))
    def pointer(root):
        r,child=C.c_ulong(),C.c_ulong();rx,ry,wx,wy=C.c_int(),C.c_int(),C.c_int(),C.c_int();mask=C.c_uint()
        same=x.XQueryPointer(connection,root,C.byref(r),C.byref(child),C.byref(rx),C.byref(ry),C.byref(wx),C.byref(wy),C.byref(mask))
        return bool(same),rx.value,ry.value,mask.value
    def read(child,size):
        data=bytearray();deadline=time.monotonic()+5
        while len(data)<size:
            assert time.monotonic()<deadline,'X11 input reply timed out'
            if select.select([child.stdout],[],[],.02)[0]:
                part=os.read(child.stdout.fileno(),size-len(data))
                assert part,'X11 input failed: '+child.stderr.read().decode()
                data.extend(part)
        return bytes(data)
    def open_input(screen):
        child=subprocess.Popen([str(worker),'input',screen['id']],env=env,
            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
        children.append(child)
        child.stdin.write(struct.pack('<iiII',screen['x'],screen['y'],screen['desktop_width'],screen['desktop_height']))
        assert read(child,4)==struct.pack('<I',0x31424150)
        return child
    def send(child,kind,xv=0,yv=0,value=0,down=False):
        child.stdin.write(struct.pack('<BHHHB',kind,xv,yv,value&65535,down))
        assert read(child,1)==b'\1'
    def recover():
        result=subprocess.run([str(worker),'recover-input'],env=env,capture_output=True,timeout=5)
        assert result.returncode==0,result.stderr.decode()
    try:
        for screen in screens:
            number=int(screen['id'].split(':')[1]);root=x.XRootWindow(connection,number)
            width,height=screen['pixel_width'],screen['pixel_height']
            window=x.XCreateSimpleWindow(connection,root,0,0,width,height,0,0,0)
            assert window;windows.append(window)
            x.XSelectInput(connection,window,1|2|4|8);x.XMapRaised(connection,window)
            x.XSetInputFocus(connection,window,2,0);x.XSync(connection,False);events()
            child=open_input(screen)
            send(child,1,65535,65535)
            same,px,py,mask=pointer(root)
            assert same and (px,py)==(width-1,height-1), {'screen':screen['id'],'pointer':[same,px,py,mask],'expected':[width-1,height-1]}
            send(child,1,0,0);assert pointer(root)[1:3]==(0,0)
            # Exactly one down/up, even if duplicate downs arrive.
            send(child,2,32768,32768,0,True);send(child,2,32768,32768,0,True)
            assert pointer(root)[3]&256
            send(child,2,32768,32768,0,False)
            observed=events();button=[e for e in observed if e[0] in (4,5)]
            assert [e[:2] for e in button]==[[4,1],[5,1]],button
            expected=(round((width-1)*32768/65535),round((height-1)*32768/65535))
            assert all(tuple(e[2:])==expected for e in button)
            send(child,4,value=0xa2,down=True);send(child,4,value=0x41,down=True)
            assert key_down(37) and key_down(38)
            send(child,4,value=0x41,down=False);send(child,4,value=0xa2,down=False)
            assert not key_down(37) and not key_down(38)
            key_events=[e[:2] for e in events() if e[0] in (2,3)]
            assert key_events==[[2,37],[2,38],[3,38],[3,37]],key_events
            send(child,3,value=60);assert not [e for e in events() if e[0] in (4,5)]
            send(child,3,value=60);send(child,3,value=-120)
            assert [e[:2] for e in events() if e[0] in (4,5)]==[[4,4],[5,4],[4,5],[5,5]]
            send(child,4,value=0xa2,down=True);send(child,2,100,100,2,True)
            send(child,5);assert not key_down(37) and not pointer(root)[3]&(256|512|1024)
            child.stdin.close();child.wait(timeout=5);assert child.returncode==0
            recover();events()
            endings=[]
            for ending in ('normal_eof','partial_eof','invalid_packet','sigkill'):
                child=open_input(screen)
                send(child,4,value=0xa2,down=True);send(child,2,100,100,0,True)
                assert key_down(37) and pointer(root)[3]&256
                # Concurrent recovery cannot disturb the live worker's keys.
                busy=subprocess.run([str(worker),'recover-input'],env=env,capture_output=True,timeout=5)
                assert busy.returncode!=0 and key_down(37)
                if ending=='sigkill':child.kill()
                elif ending=='invalid_packet':child.stdin.write(bytes([255])+bytes(7))
                else:
                    if ending=='partial_eof':child.stdin.write(bytes([4,0,0]))
                    child.stdin.close()
                child.wait(timeout=5)
                recover()
                assert not key_down(37) and not pointer(root)[3]&256,ending
                endings.append(ending);events()
            results.append({'screen':screen['id'],'exact_center':list(expected),'edge_mapping':True,
                'button_down_up_once':True,'ctrl_a_down_up':True,'wheel_accumulation':True,
                'release_all':True,'recovered':endings,'live_owner_exclusion':True})
        state_dir=pathlib.Path(env['XDG_RUNTIME_DIR'])/'pixels-remote-input'
        files=list(state_dir.glob('*.releases'));assert len(files)==1
        assert files[0].read_bytes()==bytes(261)
        assert files[0].stat().st_mode&0o077==0
        print(json.dumps({'x11_input_passed':True,'screens':results,'remaining_release_flags':0}),flush=True)
    finally:
        for child in children:
            if child.poll() is None:child.kill()
            child.wait(timeout=5)
        recover()
        for window in windows:x.XDestroyWindow(connection,window)
        x.XCloseDisplay(connection)
