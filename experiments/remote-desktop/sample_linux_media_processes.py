import json,pathlib,time,sys
stage=pathlib.Path(sys.argv[1]);fixture_path=pathlib.Path(sys.argv[2])
assert stage.parent==fixture_path.parent==pathlib.Path('/tmp') and stage.name.startswith('pab-formal-linux-') and fixture_path.name.startswith('pab-linux-viewer-')
assert stage.is_dir() and not stage.is_symlink()
rows=[]
for p in pathlib.Path('/proc').iterdir():
 if not p.name.isdecimal():continue
 try:
  if (p/'exe').resolve().parent!=stage:continue
  args=(p/'cmdline').read_bytes().split(b'\0')[1:]
  mode=next((x.decode() for x in args if x in (b'--media-supervisor',b'--media-helper',b'--media-worker')), 'executor')
  worker=args[args.index(b'--media-worker')+1].decode() if b'--media-worker' in args else None
  status=dict(x.split(':',1) for x in (p/'status').read_text().splitlines() if ':' in x)
  rows.append({'pid':int(p.name),'mode':mode,'worker':worker,'fds':len(list((p/'fd').iterdir())),'rss_kib':int(status.get('VmRSS','0 kB').split()[0]),'threads':int(status['Threads'])})
 except (OSError,ProcessLookupError):pass
fixture=json.loads((fixture_path/'fixture.json').read_text())
assert time.time()-fixture['updated']<2
print(json.dumps({'processes':rows,'fixture':fixture,'restored':(stage/'state/restored.json').exists()}))
