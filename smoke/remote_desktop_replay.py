"""Replay the same bounded synthetic H.264 packet archive through native paths.

Requires an explicitly available test desktop; never run on a working desktop.
"""
import argparse
import json
from pathlib import Path
import queue
import struct
import subprocess
import threading


def packets(path):
    if Path(path).stat().st_size > 64*1024*1024:
        raise ValueError('synthetic archive exceeds 64 MiB')
    with Path(path).open('rb') as f:
        if f.read(9) != b'PABPROOF1':raise ValueError('invalid synthetic archive header')
        geometry=f.read(12)
        if len(geometry)!=12:raise ValueError('truncated geometry')
        w,h,fps=struct.unpack('<III',geometry)
        if not (0<w<=8192 and 0<h<=8192 and w*h<=33554432 and fps in (15,30)):
            raise ValueError('invalid synthetic archive geometry')
        result=[]
        while True:
            header=f.read(12)
            if not header:break
            if len(header)!=12:raise ValueError('truncated packet header')
            size,index=struct.unpack('<IQ',header)
            if not 0<size<=16*1024*1024 or index!=len(result) or len(result)>=108000:raise ValueError('invalid packet size or sequence')
            data=f.read(size)
            if len(data)!=size:raise ValueError('truncated compressed packet')
            result.append((index,data))
        if not result:raise ValueError('archive contains no complete packets')
        return (w,h,fps),result


def replay(args):
    geometry,records=packets(args.archive)
    command=[str(args.worker),'--media-worker','--smoke-pattern-proof',str(args.worker.resolve().parent),
             'render',*map(str,geometry),'stream-software' if args.software else 'stream','0','0']
    child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,
                           creationflags=0x08000020)
    events=queue.Queue()
    observed=[]
    tail=[]
    def stderr():
        for line in iter(child.stderr.readline,b''):
            tail.append(line.decode('utf-8',errors='replace').rstrip())
            del tail[:-8]
    def read():
        def take(size):
            out=b''
            while len(out)<size:
                more=child.stdout.read(size-len(out))
                if not more:raise EOFError('native replay pipe ended')
                out+=more
            return out
        def unpack(fmt):return struct.unpack(fmt,take(struct.calcsize(fmt)))
        try:
            if unpack('<II')!=(0x31424150,2):raise ValueError('native replay protocol mismatch')
            while True:
                kind=take(1)[0]
                if kind in (1,2,6):item=dict(event=kind,index=unpack('<Q')[0])
                elif kind==4:item=dict(event=kind,decoder=take(1)[0])
                elif kind==7:
                    index,decode_us,render_us=unpack('<QQQ')
                    item=dict(event=kind,index=index,decode_us=decode_us,present_call_us=render_us)
                elif kind==8:
                    index,pattern,decoded,rendered,checks=unpack('<QIIII')
                    item=dict(event=kind,index=index,pattern=pattern,decoded_error=decoded,rendered_error=rendered,checks=checks)
                elif kind==3:
                    displayed,checks,error=unpack('<QQI')
                    item=dict(event=kind,displayed=displayed,checks=checks,max_error=error)
                elif kind==5:raise ValueError(take(unpack('<H')[0]).decode('utf-8'))
                else:raise ValueError('invalid native replay event')
                observed.append(item);events.put(item)
                if kind==3:return
        except Exception as e:events.put(dict(error=str(e)))
    thread=threading.Thread(target=read,daemon=True);thread.start()
    errthread=threading.Thread(target=stderr,daemon=True);errthread.start()
    failure=None
    try:
        for index,data in records:
            child.stdin.write(struct.pack('<IQ',len(data),index)+data);child.stdin.flush()
            while True:
                event=events.get(timeout=15)
                if 'error'in event:raise RuntimeError(event['error'])
                if event.get('event')==1 and event.get('index')==index:break
        child.stdin.write(struct.pack('<I',0));child.stdin.flush()
        while True:
            event=events.get(timeout=15)
            if 'error'in event:raise RuntimeError(event['error'])
            if event.get('event')==3:break
    except Exception as e:failure=str(e)
    finally:
        try:child.stdin.close()
        except BrokenPipeError:pass
        try:child.wait(timeout=15)
        except subprocess.TimeoutExpired:child.kill();child.wait();failure=failure or 'native replay timed out'
        thread.join(timeout=2);errthread.join(timeout=2)
    proof=[e for e in observed if e.get('event')==8]
    if not failure and (len(proof)!=len(records) or child.returncode):failure='incomplete native proof replay'
    return dict(status='failed' if failure else 'passed',software_requested=args.software,
                error=failure,stderr_tail=tail,packets=len(records),checked_frames=len(proof),
                max_decoded_error=max((e['decoded_error'] for e in proof),default=None),
                max_rendered_error=max((e['rendered_error'] for e in proof),default=None),
                decoder=next((e['decoder'] for e in observed if e.get('event')==4),None),proof=proof)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--worker',type=Path,required=True);p.add_argument('--archive',type=Path,required=True)
    p.add_argument('--software',action='store_true');p.add_argument('--show-synthetic',action='store_true',required=True)
    p.add_argument('--output',type=Path,required=True)
    args=p.parse_args();result=replay(args)
    args.output.write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in result.items() if k!='proof'}))
    return int(result['status']!='passed')


if __name__=='__main__':raise SystemExit(main())
