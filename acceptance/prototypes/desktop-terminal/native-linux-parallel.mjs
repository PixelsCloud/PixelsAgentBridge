import {createRequire} from 'node:module';
import {randomBytes} from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
const require=createRequire(path.resolve('apps/desktop/package.json'));
const {chromium}=require('playwright-core');
const folder=process.env.PAB_ACCEPTANCE_FIXTURE;
const access=process.env.PAB_ACCEPTANCE_REMOTE_ACCESS;
const code=access.match(/Device code:\s*(\d+)/)[1],password=access.match(/Temporary password:\s*(\S+)/)[1];
let browser,terminal;
const result={device_code:code,checks:{}};
try{
 browser=await chromium.connectOverCDP('http://127.0.0.1:39100');
 const main=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().includes('desktopTerminal=')&&p.url().includes('tauri.localhost'));
 await main.waitForLoadState('domcontentloaded');
 await main.evaluate(()=>localStorage.setItem('pab.language','en'));
 let scope=await main.evaluate(()=>window.__TAURI_INTERNALS__.invoke('operator_current_traffic_scope'));
 if(!scope)scope=await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_register_account',args),{username:'linuxparallel_'+randomBytes(8).toString('hex'),password:randomBytes(24).toString('hex')});
 await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_connect',args),{code,password});
 await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_window',args),{code,mode:'system'});
 const deadline=Date.now()+15000;while(!terminal&&Date.now()<deadline){terminal=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('desktopTerminal='));if(!terminal)await new Promise(r=>setTimeout(r,50));}
 await terminal.getByText('Connected',{exact:true}).waitFor({timeout:30000});
 if(await terminal.getByRole('combobox').count())throw Error('System mode unexpectedly required an account chooser');
 result.checks.system_opens_automatically=true;
 let tail=Buffer.alloc(0),end;
 await terminal.exposeBinding('observeLinuxParallel',(_,frame)=>{if(frame.kind===0){tail=Buffer.concat([tail,Buffer.from(frame.data,'base64')]);if(tail.length>65536)tail=tail.subarray(-65536);}else end=JSON.parse(Buffer.from(frame.data,'base64').toString());});
 await terminal.evaluate(()=>{const a=window.__PIXELS_TERMINAL_ACCEPTANCE__;a.onFrame=buffer=>{const data=new Uint8Array(buffer);let raw='';for(const b of data.subarray(9))raw+=String.fromCharCode(b);void window.observeLinuxParallel({kind:data[0],data:btoa(raw)});};});
 await terminal.locator('.xterm-helper-textarea').focus();
 await terminal.keyboard.insertText("HISTFILE=/dev/null; for i in $(seq 1 100); do printf 'TICK_%s\\n' \"$i\"; sleep .1; done; printf '%s%s\\n' 'PARALLEL_' 'DONE'; exit 9");await terminal.keyboard.press('Enter');
 const markerUntil=Date.now()+10000;while(!tail.toString().includes('TICK_1\r\n')&&Date.now()<markerUntil)await new Promise(r=>setTimeout(r,25));
 if(!tail.toString().includes('TICK_1\r\n'))throw Error('Streaming task did not start');
 const begun=Date.now();
 const taskId=await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('operator_run_command',args),{code,program:'/bin/true',args:[],cwd:null,execution:{mode:'service'}});
 let task;const until=Date.now()+15000;
 while(Date.now()<until){task=await main.evaluate(taskId=>window.__TAURI_INTERNALS__.invoke('operator_task',{taskId,stdoutOffset:0,stderrOffset:0,encoding:null}),taskId);if(task.complete&&String(task.state).toLowerCase()==='succeeded')break;await new Promise(r=>setTimeout(r,50));}
 if(!task.complete||String(task.state).toLowerCase()!=='succeeded')throw Error('Stable command did not succeed alongside the terminal: '+task.state);
 if(end)throw Error('Stable command completed only after the streaming Shell exited');
 result.checks.stable_command_parallel={state:task.state,complete:task.complete,elapsed_ms:Date.now()-begun};
 const endedDeadline=Date.now()+20000;while(!end&&Date.now()<endedDeadline)await new Promise(r=>setTimeout(r,25));
 if(end?.exit_code!==9||!tail.toString().includes('PARALLEL_DONE'))throw Error('Streaming terminal did not finish correctly');
 result.checks.independent_terminal_continues=true;
 await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null}));
 await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy')).catch(e=>{if(!String(e).includes('has been closed'))throw e;});terminal=null;
 // Headless Linux has no local console. It must refuse Console mode, not run root.
 await main.evaluate(args=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_window',args),{code,mode:'console'});
 const consoleUntil=Date.now()+15000;while(!terminal&&Date.now()<consoleUntil){terminal=browser.contexts().flatMap(c=>c.pages()).find(p=>p.url().includes('desktopTerminal='));if(!terminal)await new Promise(r=>setTimeout(r,50));}
 await terminal.getByText('no active local console login was found',{exact:false}).waitFor({timeout:15000});
 if(await terminal.getByText('Connected',{exact:true}).count())throw Error('Console failure unexpectedly started a Shell');
 result.checks.missing_console_no_root_fallback=true;result.passed=true;
}catch(error){result.passed=false;result.error=String(error);if(terminal)result.visible_error=await terminal.locator('.desktop-terminal-alert').allTextContents().catch(()=>[]);process.exitCode=1;}
finally{if(terminal)await terminal.evaluate(()=>window.__TAURI_INTERNALS__.invoke('desktop_terminal_end',{sessionId:null})).catch(()=>{});await browser?.close();fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(result,null,2));console.log(JSON.stringify(result));}
