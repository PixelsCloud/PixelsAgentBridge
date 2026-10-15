import {createRequire} from 'node:module';
import {randomBytes,createHash} from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
const require=createRequire(path.resolve('apps/desktop/package.json'));
const {chromium}=require('playwright-core');
const folder=process.env.PAB_ACCEPTANCE_FIXTURE;
const password=process.env.PAB_ACCEPTANCE_REMOTE_ACCESS.match(/Temporary password:\s*(\S+)/)[1];
let browser,terminal;
const result={device_code:'419438177',checks:{}};
try{
 browser=await chromium.connectOverCDP('http://127.0.0.1:39100');
 const main=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().includes('desktopTerminal=')&&p.url().includes('tauri.localhost'));
 await main.waitForLoadState('domcontentloaded');
 await main.evaluate(()=>localStorage.setItem('pab.language','zh-CN'));
 let scope=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('operator_current_traffic_scope'));
 if(!scope)scope=await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_register_account',args),{username:'macprivacy_'+randomBytes(8).toString('hex'),password:randomBytes(24).toString('hex')});
 await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_connect',args),{code:'419438177',password});
 await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_window',{code:'419438177'}));
 const deadline=Date.now()+15000;while(!terminal&&Date.now()<deadline){terminal=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('desktopTerminal='));if(!terminal)await new Promise(r=>setTimeout(r,50));}
 await terminal.waitForSelector('.desktop-terminal-setup .ant-select');
 let tail=Buffer.alloc(0),received=0,end;
 const digest=createHash('sha256');
 await terminal.exposeBinding('observeMacPrivacy',(_,frame)=>{if(frame.kind===0){const data=Buffer.from(frame.data,'base64');if(frame.offset!==received)throw Error('Output gap');received+=data.length;digest.update(data);tail=Buffer.concat([tail,data]);if(tail.length>65536)tail=tail.subarray(-65536);}else end=JSON.parse(Buffer.from(frame.data,'base64').toString());});
 await terminal.evaluate(()=>{window.__PIXELS_TERMINAL_ACCEPTANCE__.onFrame=buffer=>{const data=new Uint8Array(buffer),offset=Number(new DataView(buffer).getBigUint64(1));let raw='';for(const b of data.subarray(9))raw+=String.fromCharCode(b);void window.observeMacPrivacy({kind:data[0],offset,data:btoa(raw)});};});
 await terminal.getByRole('combobox',{name:'执行账户'}).click();
 await terminal.locator('.ant-select-dropdown:visible').getByText('huayang',{exact:true}).click();
 await terminal.getByRole('button',{name:'打开 Shell',exact:true}).click();
 await terminal.getByText('已连接',{exact:true}).waitFor({timeout:30000});
 await terminal.getByText('macOS 文件访问权限',{exact:true}).waitFor();result.checks.native_permission_guidance_visible=true;
 await terminal.screenshot({path:path.join(folder,'mac-permission-guidance.png')});
 await terminal.locator('.xterm-helper-textarea').focus();
 const command="HISTFILE=/dev/null; python3 -c \"import subprocess,time; paths=['/Users/huayang/Downloads','/Users/huayang/Documents']; [subprocess.run(['/bin/ls','-U',p],stdout=subprocess.DEVNULL,check=True,timeout=5) for p in paths]; print('PROTECTED_'+'DIRS_OK')\"; printf '%s%s\\n' 'TAIL_' 'FINAL'; exit 7";
 await terminal.keyboard.insertText(command);await terminal.keyboard.press('Enter');
 const until=Date.now()+20000;while(!end&&Date.now()<until)await new Promise(r=>setTimeout(r,25));
 if(!tail.toString().includes('PROTECTED_DIRS_OK')||!end||end.exit_code!==7)throw Error('Protected directory read did not finish in the actual terminal');
 if(end.output_bytes!==received||end.sha256!==digest.copy().digest('hex'))throw Error('Mac output integrity mismatch');
 result.checks.downloads_documents_read=true;result.checks.tail_exit_source_integrity=true;
 result.output={bytes:received,sha256:end.sha256};
 await terminal.screenshot({path:path.join(folder,'mac-protected-files-result.png')});
 result.passed=true;
}catch(error){result.passed=false;result.error=String(error);process.exitCode=1;}
finally{if(terminal)await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null})).catch(()=>{});await browser?.close();fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));}
