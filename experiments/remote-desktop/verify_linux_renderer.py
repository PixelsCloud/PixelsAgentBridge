"""Generated-frame Linux rendering tests. No real screen pixels are recorded.

Reuses the platform-independent framed stream checker from the Mac tests.
Fullscreen testing temporarily changes the single VM monitor to 1080p and
restores its exact original mode/layout. Installed applications stay untouched.
"""
import argparse
import json
import pathlib
import struct
import subprocess

import verify_macos_renderer as stream


def invalid(worker, folder):
    width,height,fps,packets=stream.fixture(folder/'synthetic-15.pabvideo')
    cases=[
        ('oversize',struct.pack('<I',16777217),'packet exceeds limit',width),
        ('partial',struct.pack('<IQ',100,0)+b'123','parent ended',width),
        ('sequence',struct.pack('<IQ',len(packets[0]),1)+packets[0],'packet reordered',width),
        ('invalid-h264',struct.pack('<IQ',8,0)+b'not-h264','no valid output',width),
        ('no-idr',struct.pack('<IQ',len(packets[1]),0)+packets[1],'no valid output',width),
        ('geometry',struct.pack('<IQ',len(packets[0]),0)+packets[0],'no valid output',width-2),
    ]
    results=[]
    for name,wire,message,w in cases:
        child,timer=stream.spawn(worker,['render',str(w),str(height),str(fps),'stream','0','0'])
        try:
            # Keep the parent alive until the parser rejects the packet. Closing
            # stdin before startup only tests parent loss, not malformed video.
            out=stream.read(child.stdout,10)
            child.stdin.write(wire); child.stdin.flush()
            if name=='partial': child.stdin.close(); child.stdin=None
            child.wait(timeout=12)
            out+=child.stdout.read(); err=child.stderr.read()
            report={'case':name,'exit_code':child.returncode,'stderr':err.decode(errors='replace'),
                    'events_hex':out.hex(),'passed':False}
            report['passed']=(child.returncode != 0 and message in report['stderr'] and
                out == struct.pack('<II',0x31424150,2)+bytes([4,2]))
            results.append(report)
        finally: stream.retire(child,timer)
    return results


def full_screen(run):
    from gi.repository import Gio, GLib
    bus=Gio.bus_get_sync(Gio.BusType.SESSION,None)
    def call(method,args=None):
        return bus.call_sync('org.gnome.Mutter.DisplayConfig','/org/gnome/Mutter/DisplayConfig',
            'org.gnome.Mutter.DisplayConfig',method,args,None,Gio.DBusCallFlags.NONE,5000,None).unpack()
    before=call('GetCurrentState')
    assert len(before[1]) == len(before[2]) == 1
    spec,modes,properties=before[1][0]
    original=next(m[0] for m in modes if m[6].get('is-current'))
    target=min((m for m in modes if m[1:3] == (1920,1080) or list(m[1:3]) == [1920,1080]),key=lambda m:abs(m[3]-60))
    assert before[2][0][2] in target[5]
    def apply(mode):
        options={}
        if 'is-underscanning' in properties: options['enable_underscanning']=GLib.Variant('b',properties['is-underscanning'])
        global_options={}
        if before[3].get('supports-changing-layout-mode'): global_options['layout-mode']=GLib.Variant('u',before[3]['layout-mode'])
        configuration=[tuple(before[2][0][:5])+([(spec[0],mode,options)],)]
        call('ApplyMonitorsConfig',GLib.Variant('(uua(iiduba(ssa{sv}))a{sv})',
            (call('GetCurrentState')[0],1,configuration,global_options)))
    original_spawn=stream.spawn
    def spawn(worker,args):
        args=list(args); args[4]+='-fullscreen'
        return original_spawn(worker,args)
    result={'before_mode':original,'test_mode':target[0],'persistent':False}
    try:
        apply(target[0]); stream.spawn=spawn
        result['cases']=run()
        assert all('drawable_width=1920 drawable_height=1080' in r['stderr'] for r in result['cases'])
    finally:
        stream.spawn=original_spawn; apply(original)
        after=call('GetCurrentState')
        result['restored']=after[2]==before[2] and [m[0] for m in after[1][0][1] if m[6].get('is-current')]==[original]
        print(json.dumps(result),flush=True)
    assert result['restored']
    return result['cases']


def main():
    p=argparse.ArgumentParser()
    p.add_argument('worker',type=pathlib.Path)
    p.add_argument('--embedded',action='store_true')
    p.add_argument('--suite',choices=['rotations','fullscreen','lifecycle','redraw','invalid'],required=True)
    a=p.parse_args(); worker=a.worker.resolve(strict=True); folder=worker.parent
    assert folder.parent == pathlib.Path('/tmp') and folder.name.startswith('pab-linux-media-')
    if a.embedded:
        original_spawn=stream.spawn
        stream.spawn=lambda worker,args: original_spawn(worker,['--media-worker',*args])
    def render(rotation=0,lifecycle=False,redraw_only=False):
        return stream.render(worker,folder,'synthetic-15',rotation,True,'software',lifecycle,redraw_only)
    if a.suite=='fullscreen': results=full_screen(lambda:[render()])
    elif a.suite=='invalid': results=invalid(worker,folder)
    elif a.suite=='lifecycle': results=[render(lifecycle=True)]
    elif a.suite=='redraw': results=[render(rotation,True,True) for rotation in [0,90,180,270]]
    else: results=[render(rotation) for rotation in [0,90,180,270]]
    if a.suite!='fullscreen': print(json.dumps({'suite':a.suite,'cases':results}),flush=True)
    assert all(r['passed'] for r in results)


if __name__=='__main__': main()
