import { createRequire } from 'node:module';
import { randomBytes, createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
const require=createRequire(path.resolve('apps/desktop/package.json'));
const {chromium}=require('playwright-core');
const folder=process.env.PAB_ACCEPTANCE_FIXTURE;
const user=process.env.PAB_ACCEPTANCE_USER || 'ubuntu';
const uid=process.env.PAB_ACCEPTANCE_UID || '1000';
const largeChunks=Number(process.env.PAB_ACCEPTANCE_CHUNKS || '4097');
const access=process.env.PAB_ACCEPTANCE_REMOTE_ACCESS;
const code=access?.match(/Device code:\s*(\d+)/)?.[1];
const password=access?.match(/Temporary password:\s*(\S+)/)?.[1];
if(!code||!password) throw Error('Private target access unavailable');
const result={device_code:code,route_requested:process.env.PAB_ACCEPTANCE_ROUTE,client:'isolated native debug Desktop',started:new Date().toISOString(),sessions:[]};
let browser,main,terminal;
try {
 browser=await chromium.connectOverCDP('http://127.0.0.1:39100');
 main=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().includes('desktopTerminal=')&&p.url().includes('tauri.localhost'));
 if(!main) throw Error('Owned native main page unavailable');
 await main.waitForLoadState('domcontentloaded');
 await main.evaluate(()=>localStorage.setItem('pab.language','en'));
 let scope=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('operator_current_traffic_scope'));
 if(!scope) scope=await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_register_account',args),{username:'linuxterm_'+randomBytes(8).toString('hex'),password:randomBytes(24).toString('hex')});
 result.owned_test_account={id:scope.userId,username:scope.username};
 await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_connect',args),{code,password});
 async function open(account){
  const before=new Set(browser.contexts().flatMap(c=>c.pages()));
  await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_window',args),{code});
  const until=Date.now()+20000;
  while(Date.now()<until){ terminal=browser.contexts().flatMap(c=>c.pages()).find(p=>!before.has(p)&&p.url().includes('desktopTerminal=')); if(terminal)break; await new Promise(r=>setTimeout(r,50)); }
  if(!terminal)throw Error('Owned terminal window unavailable');
  await terminal.waitForSelector('.desktop-terminal-setup .ant-select');
  const state={received:0,tail:Buffer.alloc(0),hash:createHash('sha256'),end:null,maxPending:0};
  await terminal.exposeBinding('observeLinuxTerminal',(_,frame)=>{
   if(frame.kind===0){const data=Buffer.from(frame.data,'base64');if(frame.offset!==state.received)throw Error('Output gap or duplicate');state.received+=data.length;state.hash.update(data);state.tail=Buffer.concat([state.tail,data]);if(state.tail.length>262144)state.tail=state.tail.subarray(-262144);}
   else state.end=JSON.parse(Buffer.from(frame.data,'base64').toString());
  });
  await terminal.evaluate(()=>{
   const a=window.__PIXELS_TERMINAL_ACCEPTANCE__;if(!a)throw Error('Debug observation unavailable');
   a.onFrame=buffer=>{if(buffer.byteLength<9)return;const data=new Uint8Array(buffer),offset=Number(new DataView(buffer).getBigUint64(1));let raw='';for(const b of data.subarray(9))raw+=String.fromCharCode(b);void window.observeLinuxTerminal({kind:data[0],offset,data:btoa(raw)});};
   a.resume=()=>{a.paused=false;const queue=a.pending;a.pending=[];a.pendingBytes=0;for(const apply of queue)apply();};
  });
  await terminal.getByRole('combobox',{name:'Execution account'}).click();
  await terminal.locator('.ant-select-dropdown:visible').getByText(account==='service'?'Service account':user,{exact:true}).click();
  await terminal.getByRole('button',{name:'Open Shell',exact:true}).click();
  await terminal.getByText('Connected',{exact:true}).waitFor({timeout:30000});
  const route=await terminal.locator('.desktop-terminal-path').innerText();
  if(process.env.PAB_ACCEPTANCE_ROUTE==='relay'&&route!=='Relay')throw Error('Relay-only path was not observed');
  if(process.env.PAB_ACCEPTANCE_ROUTE==='direct'&&route!=='P2P')throw Error('Direct path was not observed');
  const check={account,route,checks:{},shell_output:{}};result.sessions.push(check);
  console.log(JSON.stringify({progress:'connected',account,route}));
  async function send(command){await terminal.locator('.xterm-helper-textarea').focus();await terminal.keyboard.insertText(command);await terminal.keyboard.press('Enter');}
  async function marker(value,timeout=20000){const until=Date.now()+timeout;while(!state.tail.toString().includes(value)&&Date.now()<until)await new Promise(r=>setTimeout(r,20));if(!state.tail.toString().includes(value))throw Error('Missing marker: '+value);}
  await send("HISTFILE=/dev/null; if [ -n \"$BASH_VERSION\" ]; then set +o history; fi; PIXELS_STATE=kept; printf '%s%s\\n' 'READY_' 'LINUX'; printf 'IDENTITY_UID_%s\\n' \"$(id -u)\"; printf 'IDENTITY_HOME_%s\\n' \"$HOME\"; printf 'IDENTITY_CWD_%s\\n' \"$PWD\"");
  await marker('READY_LINUX');await marker(account==='service'?'IDENTITY_UID_0':'IDENTITY_UID_'+uid);
  await marker(account==='service'?'IDENTITY_HOME_/root':'IDENTITY_HOME_/home/'+user);
  check.checks.identity=true;
  await send("cd /tmp; printf 'STATE_%s_DONE\\n' \"$PIXELS_STATE\"; printf '%s%s\\n' 'UNICODE_' '中文 🌍'");
  await marker('STATE_kept_DONE');await marker('UNICODE_中文 🌍');check.checks.cwd_environment_unicode=true;
  await send("python3 -c \"import os,tempfile,pathlib; p=tempfile.TemporaryDirectory(prefix='pixels-terminal-file-',dir='/tmp'); d=pathlib.Path(p.name); f=d/'unicode.txt'; f.write_text('中文 🌍'); assert f.read_text()=='中文 🌍'; assert f.stat().st_uid==os.getuid(); print('FILE_'+'CHECKED'); p.cleanup()\"");
  await marker('FILE_CHECKED');check.checks.file_access_and_ownership=true;
  await send("printf 'SIZE_BEFORE_%s_END\\n' \"$(stty size)\"");
  const sizeDeadline=Date.now()+10000;
  let beforeSize;
  while(!(beforeSize=state.tail.toString().match(/SIZE_BEFORE_(\d+) (\d+)_END/))&&Date.now()<sizeDeadline)await new Promise(r=>setTimeout(r,25));
  if(!beforeSize)throw Error('Initial PTY size unavailable');
  await terminal.setViewportSize({width:900,height:620});
  await new Promise(r=>setTimeout(r,500));
  await send("printf 'SIZE_AFTER_%s_END\\n' \"$(stty size)\"");
  const resizedDeadline=Date.now()+10000;
  let afterSize;
  while(!(afterSize=state.tail.toString().match(/SIZE_AFTER_(\d+) (\d+)_END/))&&Date.now()<resizedDeadline)await new Promise(r=>setTimeout(r,25));
  if(!afterSize||beforeSize[1]===afterSize[1]&&beforeSize[2]===afterSize[2])throw Error('Remote PTY dimensions did not change');
  check.checks.resize={before:[Number(beforeSize[1]),Number(beforeSize[2])],after:[Number(afterSize[1]),Number(afterSize[2])]};
  await send("printf '%s%s\\n' 'WAIT_' 'BEGIN'; sleep 60; printf '%s%s\\n' 'WAIT_' 'UNEXPECTED'");await marker('WAIT_BEGIN');
  await terminal.keyboard.press('Control+c');
  await send("printf '%s%s\\n' 'CTRL_C_' 'CONTINUED'");await marker('CTRL_C_CONTINUED');check.checks.ctrl_c=true;
  await terminal.evaluate(()=>window.__PIXELS_TERMINAL_ACCEPTANCE__.paused=true);
  await send("python3 -c \"import os; [os.write(1,b'P'*16384) for i in range(512)]; print('PAUSE_'+'DONE')\"");
  await new Promise(r=>setTimeout(r,1500));
  const pending=await terminal.evaluate(()=>window.__PIXELS_TERMINAL_ACCEPTANCE__.pendingBytes);
  if(pending<=0||pending>262144)throw Error('Output credit was not bounded');
  await terminal.evaluate(()=>window.__PIXELS_TERMINAL_ACCEPTANCE__.resume());await marker('PAUSE_DONE',120000);check.checks.backpressure={pending};
  const beforeLarge=state.received,started=Date.now();
  await send(`python3 -c "import os; [os.write(1,b'X'*16384) for i in range(${largeChunks})]; print('LARGE_'+'DONE')"`);
  await marker('LARGE_DONE',300000);
  if(state.received-beforeLarge<(largeChunks-1)*16384)throw Error('Large output threshold not reached');
  check.checks.large_output={bytes:state.received-beforeLarge,elapsed_ms:Date.now()-started};
  await terminal.getByRole('button',{name:'End session',exact:true}).click();
  await terminal.getByRole('button',{name:'Keep using',exact:true}).click();
  await send("printf '%s%s\\n' 'CANCEL_' 'CONTINUED'");await marker('CANCEL_CONTINUED');check.checks.cancel_close=true;
  await send("printf '%s%s\\n' 'TAIL_' 'FINAL'; exit 7");
  const deadline=Date.now()+20000;while(!state.end&&Date.now()<deadline)await new Promise(r=>setTimeout(r,50));
  if(!state.end||state.end.exit_code!==7||!state.tail.toString().includes('TAIL_FINAL'))throw Error('Exit status or tail missing');
  const hash=state.hash.copy().digest('hex');if(state.end.output_bytes!==state.received||state.end.sha256!==hash)throw Error('PTY source and renderer bytes differ');
  check.checks.source_integrity=true;check.output={bytes:state.received,sha256:hash,source:state.end};
  console.log(JSON.stringify({progress:'verified',account,bytes:state.received}));
  await terminal.screenshot({path:path.join(folder,account+'-terminal.png')});
  await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null}));
  await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy')).catch(error=>{if(!String(error).includes('has been closed'))throw error;});
  await terminal.waitForEvent('close',{timeout:3000}).catch(error=>{if(!terminal.isClosed())throw error;});
  if(main.isClosed())throw Error('Closing the terminal closed the main window');
  terminal=null;
 }
 await open('service');await open(user);
 result.passed=true;
}catch(error){result.passed=false;result.error=String(error);if(terminal)await terminal.screenshot({path:path.join(folder,'error.png')}).catch(()=>{});process.exitCode=1;}
finally{
 if(terminal)await terminal.evaluate(async()=>{await window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null});await window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy');}).catch(()=>{});
 await browser?.close();fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));
}
