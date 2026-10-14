import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import '@xterm/xterm/css/xterm.css';
import {invoke,Channel} from '@tauri-apps/api/core';
const term=new Terminal({scrollback:10000,fontSize:12});const fit=new FitAddon();term.loadAddon(fit);term.open(document.getElementById('terminal')!);fit.fit();
const report:any={kind:'native_webview_credit_proof',frames:0,received:0,consumed:0,maxPending:0,samples:[],started:Date.now(),loopsPerWave:2100};
let paused=false,queue:Uint8Array[]=[],startedPause=false,ackRunning=false,ackSent=0,ackTimer:number|null=null;
const sleep=(n:number)=>new Promise(r=>setTimeout(r,n));
window.setInterval(()=>void invoke('checkpoint',{value:{phase:report.phase,frames:report.frames,received:report.received,consumed:report.consumed,ackSent,ackRunning,paused,pauseRecovered:report.pauseRecovered}}),1000);
const b64=(s:string)=>btoa(unescape(encodeURIComponent(s)));
async function input(s:string){return invoke('control',{value:{action:'input',data:b64(s)}})}
term.onData(s=>void input(s));
function scheduleAck(){if(ackRunning||ackTimer!==null)return;ackTimer=window.setTimeout(async()=>{ackTimer=null;ackRunning=true;const offset=report.consumed;try{await invoke('ack',{offset});ackSent=offset;}finally{ackRunning=false;if(ackSent<report.consumed)scheduleAck();}},10);}
async function flushAck(){while(ackSent<report.consumed||ackRunning||ackTimer!==null){scheduleAck();await sleep(20);}}
function write(b:Uint8Array){term.write(b,()=>{report.consumed+=b.length;scheduleAck();});}
const ch=new Channel<any>();ch.onmessage=v=>{
    if(v.ended){report.ended=true;return;}
    const b=Uint8Array.from(atob(v.data),c=>c.charCodeAt(0));report.received+=b.length;report.frames++;
    report.maxPending=Math.max(report.maxPending,report.received-report.consumed);
    if(paused)queue.push(b);else write(b);
    if(report.received>1024*1024&&!startedPause){startedPause=true;void pauseTest();}
};
async function pauseTest(){
    paused=true;await invoke('visibility',{hidden:true});await sleep(2200);
    const stats:any=await invoke('admin',{value:{action:'stats'}});report.pauseStats=stats;
    if(stats.produced-stats.acked>262144)throw new Error('credit budget exceeded');
    report.samples.push(await invoke('memory'));await invoke('visibility',{hidden:false});
    paused=false;for(const b of queue)write(b);queue=[];report.pauseRecovered=true;
}
function screen(){const b=term.buffer.active;let s='';for(let i=Math.max(0,b.length-Math.max(term.rows,100));i<b.length;i++)s+=b.getLine(i)?.translateToString(true)+'\n';return s;}
async function wait(marker:string,ms=150000){const start=Date.now();while(Date.now()-start<ms){if(screen().includes(marker))return;await sleep(100);}throw new Error('missing '+marker);}
async function run(){
    report.phase='startup';
    await invoke('start',{onData:ch});await sleep(2000);report.readyInput=await input("$env:PAB_TERM_UI='kept';Write-Output ('__'+'UI_READY__')\r");await wait('__UI_READY__',20000);
    report.samples.push(await invoke('memory'));
    report.phase='first-wave';await input("$block=('W'*79+[Environment]::NewLine)*200;for($i=0;$i -lt 2100;$i++){[Console]::Write($block)};Write-Output ('__'+'UI_DONE__')\r");
    await wait('__UI_DONE__');while(report.received!==report.consumed)await sleep(100);await flushAck();
    if(report.received<=32*1024*1024||!report.pauseRecovered)throw new Error('insufficient flow test');
    const stats:any=await invoke('admin',{value:{action:'stats'}});report.serverStats=stats;
    if(stats.produced!==report.consumed||stats.acked!==report.consumed||stats.max_inflight>262144)throw new Error('byte accounting mismatch');
    report.warmedLines=term.buffer.active.length;report.heapWarm=(performance as any).memory?.usedJSHeapSize;
    report.phase='second-wave';await input("for($i=0;$i -lt 2100;$i++){[Console]::Write($block)};Write-Output ('__'+'SECOND_DONE__')\r");await wait('__SECOND_DONE__');
    while(report.received!==report.consumed)await sleep(100);await flushAck();
    report.secondLines=term.buffer.active.length;report.heapAfter=(performance as any).memory?.usedJSHeapSize;
    if(report.secondLines>10000+term.rows)throw new Error('unbounded scrollback');
    report.phase='recovery';await input("Write-Output ('STATE:'+$env:PAB_TERM_UI);Write-Output ('__'+'UI_RECOVERED__')\r");await wait('__UI_RECOVERED__',15000);
    if(!screen().includes('STATE:kept'))throw new Error('shell state lost');
    while(report.received!==report.consumed)await sleep(100);await flushAck();report.finalStats=await invoke('admin',{value:{action:'stats'}});
    report.samples.push(await invoke('memory'));await invoke('admin',{value:{action:'shutdown'}});await sleep(300);
    report.success=true;report.elapsedMs=Date.now()-report.started;await invoke('finish',{value:report});
}
run().catch(async e=>{report.error=String(e);report.screenTail=screen().slice(-3000);report.cols=term.cols;report.rows=term.rows;try{report.failedStats=await invoke('admin',{value:{action:'shutdown'}})}catch{}await invoke('finish',{value:report});});
