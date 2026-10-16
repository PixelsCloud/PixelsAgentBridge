import { createRoot } from 'react-dom/client';
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
const fixture=(window as any).fixture={calls:[] as any[],cancel:null as null|(()=>void),signedIn:false};
const connectionFailure = new URLSearchParams(location.search).get('connection-error');
(window as any).__TAURI_INTERNALS__={transformCallback:()=>1,unregisterCallback:()=>{},invoke:async(command:string,args:any)=>{
  fixture.calls.push({command,args});
  if(command==='operator_bootstrap')return {...history,devices};
  if(command==='operator_connect_saved'){
    if(connectionFailure)throw connectionFailure;
    return {...devices[0],connected:true};
  }
  if(command==='operator_connect' && location.search.includes('expired'))throw 'account unavailable; sign in again before controlling devices';
  if(command==='operator_connect'){
    if(args.password !== 'correct-device-password')throw 'the device rejected authentication';
    return {...devices[0],connected:true};
  }
  if(command==='operator_history_page')return history;
  if(command==='operator_rename_device'){devices[0].alias=args.alias.trim();return devices[0].alias;}
  if(command==='github_enabled')return true;
  if(command==='github_login')return new Promise((_,reject)=>{fixture.cancel=()=>reject('github_cancelled');});
  if(command==='github_cancel'){fixture.cancel?.();return;}
  if(command==='plugin:event|listen')return 1;
  return [];
}};
createRoot(document.getElementById('root')!).render(<ConfigProvider><App>
  {location.search.includes('github')?<GithubAccount language="en" onSignedIn={()=>{fixture.signedIn=true;}}/>:<OperatorPanel signedIn={!location.search.includes("logged-out")} onSignIn={()=>{fixture.signedIn=true;}} language="en" view={/logged-out|expired|manual/.test(location.search) ? "home" : "remote"} onOpenRemote={()=>{}}/>}
</App></ConfigProvider>);
