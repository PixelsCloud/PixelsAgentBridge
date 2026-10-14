import {createRequire} from 'node:module';
import {createHash} from 'node:crypto';
import fs from 'node:fs';import path from 'node:path';
const fixture=process.env.PAB_ACCEPTANCE_FIXTURE;
const {chromium}=createRequire(process.env.PAB_ACCEPTANCE_NODE_PACKAGE)('playwright-core');
const code=process.env.PAB_ACCEPTANCE_TARGET;
const result={controller:process.env.PAB_ACCEPTANCE_CONTROLLER,device:code,checks:{},started:new Date().toISOString()};
const browser=await chromium.connectOverCDP('http://127.0.0.1:39100');let pages=[];
try {
  const main=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('tauri.localhost')&&!p.url().includes('desktopTerminal='));
  const invoke=(command,args={})=>main.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  await invoke('operator_connect_saved',{code});
  async function open(){const old=new Set(browser.contexts().flatMap(c=>c.pages()));await invoke('desktop_terminal_window',{code});let page;const until=Date.now()+30000;while(!page&&Date.now()<until){page=browser.contexts().flatMap(c=>c.pages()).find(p=>!old.has(p)&&p.url().includes('desktopTerminal='));if(!page)await new Promise(r=>setTimeout(r,30));}if(!page)throw new Error('window missing');await page.waitForSelector('.desktop-terminal-setup');pages.push(page);return page;}
  async function destroy(page){let failure;try{await page.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy'));}catch(error){failure=error;}const until=Date.now()+10000;while(!page.isClosed()&&Date.now()<until)await new Promise(r=>setTimeout(r,20));if(!page.isClosed())throw failure||new Error('owned window did not close');}
  for(let i=0;i<8;i++)await open();
  let denied=false;try{await invoke('desktop_terminal_window',{code});}catch{denied=true;}if(!denied)throw new Error('ninth window was accepted');
  result.checks.eight_window_limit=true;
  const terminal=pages[0];
  const prevented=await terminal.evaluate(async()=>{try{await window.__TAURI_INTERNALS__.invoke('plugin:window|destroy',{label:'main'});return false;}catch{return true;}});
  if(!prevented)throw new Error('terminal could destroy the main window');result.checks.cross_window_destroy_denied=true;
  if(main.isClosed())throw new Error('main window was closed during a denied operation');
  const mainRejected=await main.evaluate(async()=>{try{await window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy');return false;}catch{return true;}});
  if(!mainRejected)throw new Error('main window was accepted as a terminal');
  result.checks.main_window_isolation=true;
  await destroy(terminal);pages.shift();
  const replacement=await open();result.checks.closed_window_releases_slot=true;
  // Keep only two windows so selection, independent close, and latency can be
  // tested without incidental memory from unused WebViews.
  for(const p of [...pages].slice(1,-1)){await destroy(p);pages=pages.filter(v=>v!==p);}
  const shell=pages[0];let tail=Buffer.alloc(0),count=0,end=null;const digest=createHash('sha256');
  await shell.exposeBinding('windowFrame',(_,frame)=>{const b=Buffer.from(frame.data,'base64');if(frame.kind===0){if(frame.offset!==count)throw new Error('stream gap');count+=b.length;digest.update(b);tail=Buffer.concat([tail,b]);if(tail.length>65536)tail=tail.subarray(tail.length-65536);}else end=JSON.parse(b.toString());});
  await shell.evaluate(()=>{window.__PIXELS_TERMINAL_ACCEPTANCE__.onFrame=b=>{const bytes=new Uint8Array(b);let s='';for(const v of bytes.subarray(9))s+=String.fromCharCode(v);void window.windowFrame({kind:bytes[0],offset:Number(new DataView(b).getBigUint64(1)),data:btoa(s)});};});
  await shell.getByRole('combobox',{name:'Execution account'}).click();await shell.locator('.ant-select-dropdown:visible').getByText('Service account',{exact:true}).click();await shell.getByRole('button',{name:'Open Shell',exact:true}).click();await shell.getByText('Connected',{exact:true}).waitFor();
  async function send(text){await shell.locator('.xterm-helper-textarea').focus();await shell.keyboard.insertText(text);await shell.keyboard.press('Enter');}
  async function marker(value){const until=Date.now()+15000;while(!tail.toString().includes(value)&&Date.now()<until)await new Promise(r=>setTimeout(r,5));if(!tail.toString().includes(value))throw new Error('missing '+value);}
  await send("if(Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue){Set-PSReadLineOption -HistorySaveStyle SaveNothing}; [Console]::WriteLine(('WINDOW_'+'READY'))");await marker('WINDOW_READY');
  const latency=[];for(let i=0;i<30;i++){const started=Date.now();await send("[Console]::WriteLine(('LAT_'+'"+i+"'+'_DONE'))");await marker('LAT_'+i+'_DONE');latency.push(Date.now()-started);}
  const sorted=[...latency].sort((a,b)=>a-b);result.checks.quiet_input_latency={samples_ms:latency,p95_ms:sorted[Math.ceil(sorted.length*.95)-1]};
  await shell.evaluate(async()=>{const id=window.__PIXELS_TERMINAL_ACCEPTANCE__.sessionId;await window.__TAURI_INTERNALS__.invoke('desktop_terminal_resize',{sessionId:id,cols:120,rows:45});});
  result.checks.resize=true;
  await destroy(replacement);pages=pages.filter(p=>p!==replacement);
  await send("[Console]::WriteLine(('OTHER_'+'WINDOW_CLOSED')); exit 13");await marker('OTHER_WINDOW_CLOSED');
  const until=Date.now()+30000;while(!end&&Date.now()<until)await new Promise(r=>setTimeout(r,20));if(!end||end.exit_code!==13||end.output_bytes!==count||end.sha256!==digest.copy().digest('hex'))throw new Error('independent close or EOF integrity failed');
  result.checks.other_window_close_keeps_shell=true;
  await shell.screenshot({path:path.join(fixture,'native-terminal-final.png')});
  // Cleanup after EOF is part of acceptance: do not hide a failed close in
  // finally, where best-effort recovery is appropriate only after a test error.
  await destroy(shell);pages=pages.filter(p=>p!==shell);
  result.checks.natural_exit_window_close=true;
  result.connection_paths=await invoke('operator_connection_paths');result.finished=new Date().toISOString();
}catch(error){result.error=String(error);process.exitCode=1;}
finally{for(const p of pages)await p.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy')).catch(()=>{});fs.writeFileSync(path.join(fixture,'native-window-check.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));await browser.close();}
