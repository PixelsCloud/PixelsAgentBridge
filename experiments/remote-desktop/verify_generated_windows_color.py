"""Present generated H.264 packets through default and forced software paths.

Creates only an owned Tk host and native media child. Samples RGB values from
its visible surface, never saves desktop images or sends input. Windows only.
Default mode may fall back to software; this probe does not prove hardware use.
"""
import argparse, ctypes, hashlib, json, pathlib, queue, struct, subprocess, threading, time
from ctypes import wintypes
import tkinter as tk

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument('desktop',type=pathlib.Path);ap.add_argument('packets',type=pathlib.Path)
    ap.add_argument('result',type=pathlib.Path);args=ap.parse_args()
    data=args.packets.read_bytes();assert len(data)<=16*1024*1024
    packets=[];offset=0
    while offset<len(data):
        size=struct.unpack_from('<I',data,offset)[0];offset+=4
        assert 0<size<=16*1024*1024 and offset+size<=len(data)
        packets.append(data[offset:offset+size]);offset+=size
    assert len(packets)==30
    u=ctypes.WinDLL('user32',use_last_error=True);g=ctypes.WinDLL('gdi32',use_last_error=True)
    u.SetProcessDpiAwarenessContext.argtypes=[wintypes.HANDLE];u.SetProcessDpiAwarenessContext(wintypes.HANDLE(-4))
    callback=ctypes.WINFUNCTYPE(wintypes.BOOL,wintypes.HWND,wintypes.LPARAM)
    u.EnumChildWindows.argtypes=[wintypes.HWND,callback,wintypes.LPARAM]
    u.GetClassNameW.argtypes=[wintypes.HWND,wintypes.LPWSTR,ctypes.c_int]
    u.GetWindowThreadProcessId.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.DWORD)]
    u.GetClientRect.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.RECT)]
    u.ClientToScreen.argtypes=[wintypes.HWND,ctypes.POINTER(wintypes.POINT)]
    u.GetDC.argtypes=[wintypes.HWND];u.GetDC.restype=wintypes.HDC
    u.ReleaseDC.argtypes=[wintypes.HWND,wintypes.HDC]
    g.GetPixel.argtypes=[wintypes.HDC,ctypes.c_int,ctypes.c_int];g.GetPixel.restype=wintypes.DWORD
    report={'desktop_sha256':hashlib.sha256(args.desktop.read_bytes()).hexdigest(),
            'packets_sha256':hashlib.sha256(data).hexdigest(),'runs':[]}
    for mode in ('normal','normal-software'):
        host=tk.Tk();host.title('Pixels generated color acceptance');host.geometry('1100x800+40+40')
        host.attributes('-topmost',True);host.update();parent=host.winfo_id()
        p=subprocess.Popen([str(args.desktop.resolve()),'--media-worker','render','1280','800','15',mode,str(parent),'0'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,creationflags=subprocess.CREATE_NO_WINDOW)
        events=queue.Queue();go=threading.Event();stop=threading.Event()
        def read(n):
            out=b''
            while len(out)<n:
                part=p.stdout.read(n-len(out));assert part,'Media pipe closed';out+=part
            return out
        def drive():
            try:
                assert read(4)==struct.pack('<I',0x31424150)
                for index,packet in enumerate(packets):
                    if stop.is_set():return
                    p.stdin.write(struct.pack('<IQ',len(packet),index)+packet);p.stdin.flush()
                    assert read(8)==struct.pack('<Q',index)
                    events.put(('frame',index));assert go.wait(5),'Visible sampling timed out';go.clear()
                p.stdin.write(struct.pack('<I',0));p.stdin.flush();p.stdin.close()
                tail=p.stdout.read();events.put(('done',{'tail_bytes':len(tail),'displayed':struct.unpack_from('<Q',tail)[0] if len(tail)>=8 else None}))
            except BaseException as error:events.put(('error',repr(error)))
        thread=threading.Thread(target=drive,daemon=True);thread.start();samples=[];deadline=time.monotonic()+45
        try:
            while True:
                host.update();assert time.monotonic()<deadline,'Renderer observation timeout'
                try:kind,value=events.get(timeout=.01)
                except queue.Empty:continue
                if kind=='error':raise RuntimeError(value)
                if kind=='done':summary=value;break
                time.sleep(.08);host.update();found=[]
                @callback
                def child(hwnd,_):
                    name=ctypes.create_unicode_buffer(100);pid=wintypes.DWORD()
                    u.GetClassNameW(hwnd,name,100);u.GetWindowThreadProcessId(hwnd,ctypes.byref(pid))
                    if name.value=='PixelsRemoteDesktopSurface' and pid.value==p.pid:found.append(hwnd)
                    return True
                u.EnumChildWindows(parent,child,0);assert len(found)==1
                bounds=wintypes.RECT();origin=wintypes.POINT();assert u.GetClientRect(found[0],ctypes.byref(bounds)) and u.ClientToScreen(found[0],ctypes.byref(origin))
                w,h=bounds.right,bounds.bottom;scale=min(w/1280,h/800);left=(w-int(1280*scale))//2;top=(h-int(800*scale))//2
                dc=u.GetDC(None);assert dc
                try:
                    points=[]
                    for y in (800//3,400,600):
                        for x in (320,640,960):
                            color=g.GetPixel(dc,origin.x+left+int(x*scale),origin.y+top+int(y*scale));assert color!=0xffffffff
                            points.append([color&255,(color>>8)&255,(color>>16)&255])
                finally:u.ReleaseDC(None,dc)
                samples.append({'index':value,'rgb':points});go.set()
            p.wait(timeout=5);assert p.returncode==0,p.stderr.read().decode(errors='replace')
            report['runs'].append({'mode':mode,'samples':samples,'summary':summary,'exit_code':p.returncode})
            args.result.write_text(json.dumps(report,indent=2))
        finally:
            stop.set();go.set()
            if p.poll() is None:p.kill();p.wait(timeout=5)
            thread.join(timeout=5);host.destroy()
    print(json.dumps({'result':str(args.result),'runs':[{'mode':r['mode'],'frames':len(r['samples']),'first_rgb':r['samples'][0]['rgb']} for r in report['runs']]}))

if __name__=='__main__':main()
