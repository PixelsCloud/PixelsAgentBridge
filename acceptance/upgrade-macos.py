"""One-shot installation observer; run as root from an independent launchd job."""
import hashlib,json,os,plistlib,signal,subprocess,sys,time
from pathlib import Path

pkg=Path(sys.argv[1]);expected=sys.argv[2];version=sys.argv[3];root=Path(sys.argv[4])
(root/'started').open('x').close()
report={'state':'starting','version':version,'identity_preserved':False}
app=Path('/Applications/Pixels Agent Bridge.app')
data=Path('/Library/Application Support/PixelsAgentBridgeData')
install=Path('/Library/Application Support/PixelsAgentBridge')
def digest(p):
    with p.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
def requirement():
    r=subprocess.run(['/usr/bin/codesign','-d','-r-',str(app)],capture_output=True,text=True,check=True)
    return next(line for line in (r.stdout+r.stderr).splitlines() if line.startswith('designated =>'))
try:
    assert digest(pkg)==expected,'Installer hash mismatch'
    before=digest(data/'device-endpoint.key');signature=requirement()
    desktop=str(app/'Contents/MacOS/pab-desktop')
    for row in subprocess.check_output(['/bin/ps','-axo','pid=,command='],text=True).splitlines():
        pid,command=row.strip().split(None,1)
        if command==desktop:os.kill(int(pid),signal.SIGTERM)
    time.sleep(2)
    report['state']='installing';(root/'result.json').write_text(json.dumps(report))
    with (root/'installer.log').open('w') as log:
        run=subprocess.run(['/usr/sbin/installer','-pkg',str(pkg),'-target','/'],stdout=log,stderr=subprocess.STDOUT)
    report['exit_code']=run.returncode
    assert run.returncode==0,'Installer failed; inspect installer.log'
    assert plistlib.loads((app/'Contents/Info.plist').read_bytes())['CFBundleShortVersionString']==version
    subprocess.run(['/usr/bin/codesign','--verify','--deep','--strict',str(app)],check=True)
    assert requirement()==signature,'Signing identity changed'
    report['signature_preserved']=True
    assert digest(data/'device-endpoint.key')==before,'Device identity changed'
    report['identity_preserved']=True
    report['hashes']={p.name:digest(p) for p in [install/'pab-executor',install/'pab-mcp',app/'Contents/MacOS/pab-desktop']}
    report['state']='succeeded'
except Exception as error:
    report['state']='failed';report['error']=str(error)
finally:
    report['finished_at']=time.time();(root/'result.json').write_text(json.dumps(report,indent=2))
sys.exit(0 if report['state']=='succeeded' else 1)
