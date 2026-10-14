// Cross-device, bounded-memory native acceptance. Synthetic inputs only.
import { createRequire } from 'node:module';
import { createHash, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
const fixture=process.env.PAB_ACCEPTANCE_FIXTURE;
const {chromium}=createRequire(process.env.PAB_ACCEPTANCE_NODE_PACKAGE)('playwright-core');
const code=process.env.PAB_ACCEPTANCE_TARGET;
const result={controller:process.env.PAB_ACCEPTANCE_CONTROLLER,device:code,started:new Date().toISOString(),checks:{},samples:[]};
const report=()=>fs.writeFileSync(path.join(fixture,'native-mixed.json'),JSON.stringify(result,null,2));
let browser,main,shell,stableId,timer;
const clients=[];
try {
  browser=await chromium.connectOverCDP('http://127.0.0.1:39100');
  main=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().includes('desktopTerminal=')&&p.url().includes('tauri.localhost'));
  if(!main)throw new Error('owned main window unavailable');
  const invoke=(command,args={})=>main.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  await invoke('operator_connect_saved',{code});
  async function open() {
    const before=new Set(browser.contexts().flatMap(c=>c.pages()));
    await invoke('desktop_terminal_window',{code});
    const deadline=Date.now()+30000;let page;
    while(!page&&Date.now()<deadline){page=browser.contexts().flatMap(c=>c.pages()).find(p=>!before.has(p)&&p.url().includes('desktopTerminal=')); if(!page)await new Promise(r=>setTimeout(r,50));}
    if(!page)throw new Error('new window missing');
    clients.push(page);await page.waitForSelector('.desktop-terminal-setup');return page;
  }
  shell=await open();
  const contexts=await shell.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_accounts'));
  const user=contexts.data.entries.find(e=>e.mode==='user'&&e.selection&&e.account_name.toLowerCase().endsWith('\\hy'));
  if(!user)throw new Error('Company native hy context unavailable');
  result.identity=user.identity;
  let count=0,tail=Buffer.alloc(0),ended=null;const digest=createHash('sha256');
  await shell.exposeBinding('mixedFrame',(_,frame)=>{
    const bytes=Buffer.from(frame.bytes,'base64');
    if(frame.kind===0){if(frame.offset!==count)throw new Error('gap or duplicate');count+=bytes.length;digest.update(bytes);tail=Buffer.concat([tail,bytes]);if(tail.length>65536)tail=tail.subarray(tail.length-65536);}
    else ended=JSON.parse(bytes.toString());
  });
  await shell.evaluate(()=>{window.__PIXELS_TERMINAL_ACCEPTANCE__.onFrame=buffer=>{const b=new Uint8Array(buffer);let s='';for(const v of b.subarray(9))s+=String.fromCharCode(v);void window.mixedFrame({kind:b[0],offset:Number(new DataView(buffer).getBigUint64(1)),bytes:btoa(s)});};});
  await shell.getByRole('combobox',{name:'Execution account'}).click();
  const label=user.account_name+(user.session_id?' · '+user.session_id:'');
  await shell.locator('.ant-select-dropdown:visible').getByText(label,{exact:true}).click();
  await shell.getByRole('button',{name:'Open Shell',exact:true}).click();
  await shell.getByText('Connected',{exact:true}).waitFor();
  async function send(text){await shell.locator('.xterm-helper-textarea').focus();await shell.keyboard.insertText(text);await shell.keyboard.press('Enter');}
  async function marker(value,timeout=30000){const deadline=Date.now()+timeout;while(!tail.toString().includes(value)&&Date.now()<deadline)await new Promise(r=>setTimeout(r,20));if(!tail.toString().includes(value))throw new Error('marker missing: '+value);}
  await send("if(Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue){Set-PSReadLineOption -HistorySaveStyle SaveNothing}; [Console]::WriteLine(('USER_'+[Environment]::UserName+'_READY'))");await marker('USER_hy_READY');
  result.checks.native_user=true;
  // Terminal windows cannot invoke the main window's task APIs.
  const denied=await shell.evaluate(async()=>{try{await window.__TAURI_INTERNALS__.invoke('operator_run_command',{code:'559038160',program:'cmd.exe',args:['/c','echo NEVER'],cwd:null,execution:null});return false;}catch{return true;}});
  if(!denied)throw new Error('terminal window escaped its invocation boundary');
  result.checks.window_permissions=true;
  const markerOnly='HUMAN_ONLY_'+randomUUID().replaceAll('-','');
  await send("$env:PIXELS_HUMAN_ONLY='"+markerOnly+"'; [Console]::WriteLine(('PRIVATE_'+'READY'))");await marker('PRIVATE_READY');
  const stable=await invoke('operator_open_terminal',{code,cols:100,rows:30,execution:{mode:'service'}});stableId=stable.sessionId;
  // Exercise the already stable terminal alongside the independent human PTY.
  const stableCommand=stable.shell.toLowerCase().includes('powershell')?"Write-Output ('STABLE_'+'MIXED_OK')":"echo STABLE_^MIXED_OK";
  // The stable terminal already answers its own initial ConPTY cursor query.
  await invoke('operator_terminal_input',{id:stableId,data:Buffer.from(stableCommand+"\r").toString('base64')});
  let stableText='';for(let i=0;i<40&&!stableText.includes('STABLE_MIXED_OK');i++){const r=await invoke('operator_terminal_read',{id:stableId});stableText+=Buffer.from(r.data,'base64').toString();if(!stableText.includes('STABLE_MIXED_OK'))await new Promise(r=>setTimeout(r,100));}
  if(!stableText.includes('STABLE_MIXED_OK'))throw new Error('stable terminal unavailable');
  result.checks.stable_terminal_parallel=true;
  const metrics=await shell.context().newCDPSession(shell);
  async function sample(){const heap=await metrics.send('Runtime.getHeapUsage');result.samples.push({elapsed_ms:Date.now()-started,bytes:count,heap_used:heap.usedSize});report();}
  const started=Date.now();await sample();
  timer=setInterval(()=>void sample().catch(e=>{result.sample_error=String(e);}),15000);
  await send("$deadline=[DateTime]::UtcNow.AddSeconds(600); $s='Z'*65536; while([DateTime]::UtcNow -lt $deadline){[Console]::Write($s); Start-Sleep -Milliseconds 50}; [Console]::WriteLine(('MIXED_'+'FINISHED'))");
  // Probe stable command and file APIs on the SAME runtime while output flows.
  const latencies=[];
  for(let i=0;i<12;i++){
    const begin=Date.now();
    const task=await invoke('operator_run_command',{code,program:'cmd.exe',args:['/c','echo PARALLEL_COMMAND_OK'],cwd:null,execution:{mode:'service'}});
    let update;const until=Date.now()+30000;
    do{update=await invoke('operator_task',{taskId:task,stdoutOffset:0,stderrOffset:0,encoding:null});if(['Succeeded','Failed','Cancelled'].includes(update.state))break;await new Promise(r=>setTimeout(r,100));}while(Date.now()<until);
    if(update.state!=='Succeeded')throw new Error('parallel command did not succeed: '+JSON.stringify(update));
    await invoke('operator_list_directory',{code,path:'C:\\Users\\hy\\Downloads',after:null,execution:{mode:'service'}});
    latencies.push(Date.now()-begin);await new Promise(r=>setTimeout(r,1000));
  }
  result.checks.command_and_file_parallel={count:latencies.length,max_ms:Math.max(...latencies)};
  await marker('MIXED_FINISHED',720000);
  clearInterval(timer);timer=null;await sample();
  const duration=Date.now()-started;if(duration<600000)throw new Error('short duration was represented as a ten-minute acceptance');
  result.checks.ten_minute_stream={duration_ms:duration,bytes:count};
  await send("[Console]::WriteLine(('USER_'+'TAIL')); exit 9");
  const until=Date.now()+30000;while(!ended&&Date.now()<until)await new Promise(r=>setTimeout(r,20));
  if(!ended||ended.exit_code!==9||ended.output_bytes!==count||ended.sha256!==digest.copy().digest('hex'))throw new Error('user PTY tail or integrity mismatch');
  result.checks.user_source_integrity=true;
  result.output={bytes:count,sha256:digest.copy().digest('hex'),end:ended};
  await invoke('operator_terminal_close',{id:stableId});stableId=null;
  // Search the isolated Pixels profile, with bounded reads, for the human-only
  // marker. It must not appear in a database, history spool, or application log.
  let found=false;const needle=Buffer.from(markerOnly);
  function scan(directory){for(const item of fs.readdirSync(directory,{withFileTypes:true})){const file=path.join(directory,item.name);if(item.isDirectory()){scan(file);continue;}if(!item.isFile())continue;const fd=fs.openSync(file,'r');try{let previous=Buffer.alloc(0);const buffer=Buffer.alloc(65536);for(;;){const n=fs.readSync(fd,buffer,0,buffer.length,null);if(!n)break;const chunk=Buffer.concat([previous,buffer.subarray(0,n)]);if(chunk.includes(needle))found=true;previous=chunk.subarray(Math.max(0,chunk.length-needle.length));}}finally{fs.closeSync(fd);}}}
  scan(path.join(fixture,'ui-profile'));if(found)throw new Error('human terminal input appeared in Pixels storage');result.checks.no_human_io_storage=true;
  result.connection_paths=await invoke('operator_connection_paths');
  result.finished=new Date().toISOString();report();console.log(JSON.stringify(result));
}catch(e){result.error=String(e);report();console.error(String(e));process.exitCode=1;}
finally{
  if(timer)clearInterval(timer);
  if(stableId&&main)await main.evaluate(id=>window.__TAURI_INTERNALS__.invoke('operator_terminal_close',{id}),stableId).catch(()=>{});
  for(const page of clients)await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null})).catch(()=>{});
  await browser?.close();
}
