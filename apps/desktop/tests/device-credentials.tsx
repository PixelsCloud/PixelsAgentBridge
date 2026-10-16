import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { ConfigProvider, App } from 'antd';
import { DevicePassword } from '../src/DevicePassword';
import { currentDeviceStatus, type DeviceStatus } from '../src/localDeviceStatus';
import { messages, type Language } from '../src/i18n';
import '../src/App.css';
import '../src/PasswordMask.css';
import '../src/theme.css';

const parameters = new URLSearchParams(location.search);
const language = (parameters.get('language') ?? 'en') as Language;
const initial: DeviceStatus = {device_code:'123456789', device_id:'fixture', temporary_password:'OLDPASS8', password_version:1, executor_running:!parameters.has('offline'), control_phase:'Authenticated'};
const fixture = (window as any).fixture = {calls:[] as string[], error:parameters.has('error'), copied:'', finish:null as null|(()=>void), status:(_next:DeviceStatus)=>{}};
Object.defineProperty(navigator, 'clipboard', {value:{writeText:async(value:string)=>{fixture.copied=value;}}});
(window as any).__TAURI_INTERNALS__ = {invoke:async(command:string)=>{
  fixture.calls.push(command);
  if (command !== 'rotate_device_password') throw new Error('Unexpected fixture request');
  if (parameters.has('delay')) await new Promise<void>(resolve=>{fixture.finish=resolve;});
  if (fixture.error) throw new Error('service unavailable');
  return {...initial, temporary_password:'NEWPASS8', password_version:2};
}};
function Fixture() {
  const [device,setDevice] = useState(initial);
  const [error,setError] = useState('');
  fixture.status=(next:DeviceStatus)=>setDevice(current=>currentDeviceStatus(current,next));
  return <ConfigProvider><App><main style={{width:760,padding:16}}>
    {error && <div role="alert">{error}</div>}
    <section className="surface home-device"><div className="home-credentials">
      <div className="home-credential"><span className="code-label">{messages[language].deviceCode}</span><div className="home-credential-value"><strong>123 456 789</strong></div></div>
      <DevicePassword language={language} device={device} onRefreshed={fixture.status} onError={setError}/>
    </div></section>
  </main></App></ConfigProvider>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
