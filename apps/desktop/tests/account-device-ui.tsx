import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import type { View } from '../src/App';
import { ConfigProvider, App } from 'antd';
import { OperatorPanel } from '../src/OperatorPanel';
import { GithubAccount } from '../src/GithubAccount';
import '../src/App.css';
import '../src/theme.css';

const history={tasks:[],operations:[],totalCount:0,taskBefore:null,operationBeforeStartedAtUnixMs:null,operationBeforeId:null,hasMoreTasks:false,hasMoreOperations:false};
const devices=[{deviceId:'fixture-one',deviceCode:'123456789',alias:'Original',osFamily:'windows',osReminder:'Windows fixture',connected:false}];
if (new URLSearchParams(location.search).has('many-devices')) {
  for (let index = 1; index < 33; index++) devices.push({deviceId:`fixture-${index}`,deviceCode:String(123456789+index),alias:`Device ${index+1}`,osFamily:'windows',osReminder:'Windows fixture',connected:false});
}
const parameters = new URLSearchParams(location.search);
const presenceScenario = parameters.has('presence');
const callbacks = new Map<number, (event: unknown) => void>();
const listeners = new Map<string, number>();
const fixture=(window as any).fixture={calls:[] as any[],cancel:null as null|(()=>void),signedIn:false,
  connected:parameters.has('connected'),presence:parameters.get('presence')==='online' ? true : parameters.get('presence')==='offline' ? false : null,
  presenceError:parameters.get('presence')==='error',holdPaths:parameters.has('hold-paths'),pendingPaths:[] as {resolve:(value:unknown)=>void;reject:(error:unknown)=>void}[],
  emitConnection(phase:string){fixture.connected=phase==='connected';const handler=listeners.get('operator-device-connection');if(handler)callbacks.get(handler)?.({payload:{deviceId:'fixture-one',phase}});},
};
if(presenceScenario)devices[0].connected=fixture.connected;
const connectionFailure = new URLSearchParams(location.search).get('connection-error');
(window as any).__TAURI_INTERNALS__={transformCallback:(callback:(event:unknown)=>void)=>{const id=callbacks.size+1;callbacks.set(id,callback);return id;},unregisterCallback:()=>{},invoke:async(command:string,args:any)=>{
  fixture.calls.push({command,args});
  if(command==='operator_bootstrap')return {...history,devices};
  if(command==='operator_saved_device_presence' && presenceScenario){
    if(fixture.presenceError)throw 'control connection unavailable';
    return [{deviceCode:devices[0].deviceCode,name:'Fixture computer',online:fixture.presence}];
  }
  if(command==='operator_connection_paths' && presenceScenario){
    if(fixture.holdPaths)return new Promise((resolve,reject)=>fixture.pendingPaths.push({resolve,reject}));
    return [{deviceCode:devices[0].deviceCode,path:fixture.connected?'p2p':null}];
  }
  if(command==='operator_connect_saved'){
    if(connectionFailure)throw connectionFailure;
    fixture.connected=true;
    return {...devices[0],connected:true};
  }
  if(command==='operator_connect' && location.search.includes('expired'))throw 'account unavailable; sign in again before controlling devices';
  if(command==='operator_connect'){
    if(args.password !== 'correct-device-password')throw 'the device rejected authentication';
    fixture.connected=true;
    return {...devices[0],connected:true};
  }
  if(command==='operator_history_page')return history;
  if(command==='operator_rename_device'){devices[0].alias=args.alias.trim();return devices[0].alias;}
  if(command==='github_enabled')return true;
  if(command==='github_login')return new Promise((_,reject)=>{fixture.cancel=()=>reject('github_cancelled');});
  if(command==='github_cancel'){fixture.cancel?.();return;}
  if(command==='plugin:event|listen'){listeners.set(args.event,args.handler);return args.handler;}
  return [];
}};
function FixtureApp(){
  const [view,setView]=useState<View>(/logged-out|expired|manual/.test(location.search)||parameters.has('home')?'home':'remote');
  fixture.setView=setView;
  return <ConfigProvider><App>
    {location.search.includes('github')?<GithubAccount language="en" onSignedIn={()=>{fixture.signedIn=true;}}/>:<OperatorPanel signedIn={!location.search.includes("logged-out")} onSignIn={()=>{fixture.signedIn=true;}} language="en" view={view} onOpenRemote={()=>setView('remote')}/>}
  </App></ConfigProvider>;
}
createRoot(document.getElementById('root')!).render(<FixtureApp/>);
