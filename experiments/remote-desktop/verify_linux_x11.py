"""Owns a two-screen Xvfb; tests production X11 capture without a user's display.

Arguments: worker binary, codec-check .so, native boundary-test executable.
No system session, installed application, display manager or account is changed.
"""
import ctypes as C
import json
import os
import pathlib
import select
import struct
import subprocess
import sys
import tempfile
import time


class Bytes(C.Structure):
    _fields_ = [('data', C.POINTER(C.c_uint8)), ('size', C.c_size_t)]


def main():
    worker, codec, native = map(lambda s: pathlib.Path(s).resolve(strict=True), sys.argv[1:])
    library = C.CDLL(str(codec))
    library.pab_soft_decoder_new.argtypes = [C.c_uint32, C.c_uint32]
    library.pab_soft_decoder_new.restype = C.c_void_p
    library.pab_soft_decoder_drop.argtypes = [C.c_void_p]
    library.pab_soft_decode.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t, C.POINTER(Bytes)]
    library.pab_soft_decode.restype = C.c_bool
    read_fd, write_fd = os.pipe()
    env = os.environ.copy(); env.pop('WAYLAND_DISPLAY', None)
    env['XDG_SESSION_TYPE'] = 'x11'
    with tempfile.TemporaryDirectory(prefix='pab-x11-') as temp:
        log = open(pathlib.Path(temp)/'server.log', 'wb')
        server = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-nolisten', 'tcp',
            '-screen', '0', '1920x1080x24', '-screen', '1', '640x480x24'],
            pass_fds=(write_fd,), stdout=log, stderr=log)
        os.close(write_fd)
        try:
            assert select.select([read_fd], [], [], 10)[0], 'Xvfb did not start'
            display = os.read(read_fd, 32).decode().strip(); assert display.isdigit()
            env['DISPLAY'] = ':'+display
            boundary = subprocess.run([str(native)], env=env, capture_output=True, text=True, timeout=30)
            assert boundary.returncode == 0, boundary.stderr
            print(boundary.stdout.strip(), flush=True)
            x = C.CDLL('libX11.so.6')
            x.XOpenDisplay.argtypes=[C.c_char_p]; x.XOpenDisplay.restype=C.c_void_p
            x.XRootWindow.argtypes=[C.c_void_p,C.c_int]; x.XRootWindow.restype=C.c_ulong
            x.XSetWindowBackground.argtypes=[C.c_void_p,C.c_ulong,C.c_ulong]
            x.XClearWindow.argtypes=[C.c_void_p,C.c_ulong]
            x.XSync.argtypes=[C.c_void_p,C.c_int]; x.XCloseDisplay.argtypes=[C.c_void_p]
            connection=x.XOpenDisplay(env['DISPLAY'].encode()); assert connection
            try:
                screens=json.loads(subprocess.check_output([str(worker),'inventory'],env=env,timeout=8))
                assert len(screens)==2 and len({s['id'] for s in screens})==2
                results=[]
                def segments():
                    return {line.split()[1] for line in pathlib.Path('/proc/sysvipc/shm').read_text().splitlines()[1:]}
                baseline_segments=segments()
                for screen in screens:
                    width,height=screen['pixel_width'],screen['pixel_height']
                    number=int(screen['id'].split(':')[1]); root=x.XRootWindow(connection,number)
                    def paint(color):
                        x.XSetWindowBackground(connection,root,color); x.XClearWindow(connection,root); x.XSync(connection,False)
                    paint(0xff0000)
                    decoder=library.pab_soft_decoder_new(width,height); assert decoder
                    child=subprocess.Popen([str(worker),'capture',screen['id'],'15','desktop'],env=env,
                        stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
                    def read(size):
                        data=bytearray(); end=time.monotonic()+5
                        while len(data)<size:
                            assert time.monotonic()<end,'Capture timed out'
                            if select.select([child.stdout],[],[],.02)[0]:
                                part=os.read(child.stdout.fileno(),size-len(data))
                                assert part,'Capture failed: '+child.stderr.read().decode()
                                data.extend(part)
                        return bytes(data)
                    index=0
                    def frame():
                        nonlocal index
                        size,sequence,captured,key=struct.unpack('<IQQI',read(24))
                        assert sequence==index and 0<size<=16*1024*1024 and (index or key==1)
                        packet=read(size); decoded=Bytes()
                        assert library.pab_soft_decode(decoder,packet,size,C.byref(decoded))
                        assert decoded.size==width*height*3//2
                        index+=1
                        return decoded.data[16*width+16]
                    try:
                        assert struct.unpack('<III',read(12))==(0x31424150,width,height)
                        child.stdin.write(b'\1'); red=frame()
                        # Drain a possible initial damage event, then leave one
                        # pending request. A static desktop must not synthesize frames.
                        for _ in range(4):
                            child.stdin.write(b'\1')
                            if not select.select([child.stdout],[],[],.4)[0]:break
                            frame()
                        else:raise AssertionError('Static X11 frames repeated')
                        paint(0x00ff00); green=frame(); assert green>red+50
                        timestamps=[]; luma=[]
                        for i in range(45):
                            paint(0xff0000 if i%2 else 0x00ff00)
                            # First color can match the previous frame but XDamage
                            # still reports the repaint; alternate thereafter.
                            child.stdin.write(b'\1'); luma.append(frame());timestamps.append(time.monotonic())
                        assert all(abs(a-b)>50 for a,b in zip(luma,luma[1:]))
                        fps=44/(timestamps[-1]-timestamps[0]); assert fps>12
                        child.stdin.write(b'\0');child.wait(timeout=5);assert child.returncode==0
                        assert segments()==baseline_segments,'Shared memory survived normal close'
                        results.append({'screen':screen['id'],'dimensions':[width,height],
                            'frames':index,'capture_decode_fps':fps,'static_wait':True,'color_changes':44})
                    finally:
                        if child.poll() is None:child.kill()
                        child.wait(timeout=5);library.pab_soft_decoder_drop(decoder)
                    # Repeat with an outstanding capture request and SIGKILL.
                    # IPC_RMID must make the kernel reclaim the mapped segment
                    # after the X server notices the disconnected client.
                    for ending in ('sigkill', 'parent_eof'):
                        child=subprocess.Popen([str(worker),'capture',screen['id'],'15','desktop'],env=env,
                            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,bufsize=0)
                        try:
                            assert struct.unpack('<III',read(12))==(0x31424150,width,height)
                            child.stdin.write(b'\1');size,_,_,_=struct.unpack('<IQQI',read(24));read(size)
                            child.stdin.write(b'\1')
                            if ending=='sigkill':child.kill()
                            else:child.stdin.close()
                            child.wait(timeout=5)
                            deadline=time.monotonic()+3
                            while segments()!=baseline_segments and time.monotonic()<deadline:time.sleep(.05)
                            assert segments()==baseline_segments,'Shared memory survived '+ending
                            results[-1][ending+'_reclaimed_shared_memory']=True
                        finally:
                            if child.poll() is None:child.kill()
                            child.wait(timeout=5)
                print(json.dumps({'passed':True,'screens':results}),flush=True)
            finally:x.XCloseDisplay(connection)
        finally:
            os.close(read_fd);server.terminate()
            try:server.wait(timeout=5)
            except subprocess.TimeoutExpired:server.kill();server.wait(timeout=5)
            log.close()


if __name__=='__main__':main()
