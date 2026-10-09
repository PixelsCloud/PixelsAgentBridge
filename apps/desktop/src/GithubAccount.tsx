import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Alert, Button, Space, Typography } from 'antd';
import type { Language } from './i18n';
import type { ScopeStatus } from './operatorTypes';

const words = {
  'zh-CN': { signIn:'使用 GitHub 登录', bind:'绑定 GitHub', unlink:'解除绑定', cancel:'取消', waiting:'请在浏览器中完成 GitHub 授权', failed:'GitHub 授权未完成，请重试。', last:'这是唯一的登录方式，无法解绑。', conflict:'该 GitHub 已绑定其他账号。', expired:'授权已过期，请重试。', unavailable:'暂时无法连接 GitHub，请稍后重试。', closed:'当前不开放新账号注册。' },
  'zh-TW': { signIn:'使用 GitHub 登入', bind:'綁定 GitHub', unlink:'解除綁定', cancel:'取消', waiting:'請在瀏覽器中完成 GitHub 授權', failed:'GitHub 授權未完成，請重試。', last:'這是唯一的登入方式，無法解除綁定。', conflict:'此 GitHub 已綁定其他帳號。', expired:'授權已過期，請重試。', unavailable:'暫時無法連線 GitHub，請稍後重試。', closed:'目前不開放新帳號註冊。' },
  en: { signIn:'Continue with GitHub', bind:'Link GitHub', unlink:'Unlink', cancel:'Cancel', waiting:'Complete GitHub authorization in your browser', failed:'GitHub authorization did not finish. Please try again.', last:'Keep at least one sign-in method.', conflict:'This GitHub identity is linked to another account.', expired:'Authorization expired. Please try again.', unavailable:'GitHub is unavailable. Please try again later.', closed:'New account registration is disabled.' },
};
type Status = { enabled:boolean; login:string|null; password_enabled:boolean };

export function GithubAccount({language, signedIn=false, disabled=false, onSignedIn, onBusy}: {language:Language; signedIn?:boolean; disabled?:boolean; onSignedIn?:(scope:ScopeStatus)=>void; onBusy?:(busy:boolean)=>void}) {
  const t=words[language]; const [status,setStatus]=useState<Status>(); const [busy,setBusy]=useState(false); const [error,setError]=useState('');
  async function refresh(){
    if(signedIn)setStatus(await invoke<Status>('github_status'));
    else setStatus({enabled:await invoke<boolean>('github_enabled'),login:null,password_enabled:false});
  }
  useEffect(()=>{void refresh().catch(()=>{});return ()=>{void invoke('github_cancel').catch(()=>{});};},[signedIn]);
  function failure(e:unknown){const s=String(e);setError(s.includes('last_login_method')?t.last:s.includes('github_already_linked')?t.conflict:s.includes('github_expired')?t.expired:s.includes('github_unavailable')?t.unavailable:s.includes('registration_disabled')?t.closed:t.failed);}
  async function run(unlink=false){
    setBusy(true);onBusy?.(true);setError('');
    try {
      if(unlink)await invoke('github_unlink');
      else {const scope=await invoke<ScopeStatus>('github_login',{bind:signedIn});onSignedIn?.(scope);}
      if(signedIn)await refresh();
    }catch(e){if(!String(e).includes('github_cancelled'))failure(e);}
    finally{setBusy(false);onBusy?.(false);}
  }
  if(!status?.enabled && !status?.login)return null;
  return <Space orientation="vertical" style={{width:'100%',marginTop:12}}>
    {status?.login && <Typography.Text>GitHub: {status.login}</Typography.Text>}
    {status?.login ? <Button disabled={disabled||busy||!status.password_enabled} onClick={()=>void run(true)}>{t.unlink}</Button> : <Button block disabled={disabled&&!busy} loading={busy} onClick={()=>void run()}>{signedIn?t.bind:t.signIn}</Button>}
    {status?.login && !status.password_enabled && <Typography.Text type="secondary">{t.last}</Typography.Text>}
    {busy && <><Typography.Text type="secondary">{t.waiting}</Typography.Text><Button onClick={()=>void invoke('github_cancel')}>{t.cancel}</Button></>}
    {error && <Alert type="error" title={error} showIcon/>}
  </Space>;
}
