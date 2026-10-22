import { useEffect, useState } from 'react';
import { Alert, Button, Card, Space, Typography } from 'antd';
import { ApiError, api, post } from './api';
import { useText } from './i18n';

export type GithubStatus = { enabled:boolean; login:string|null; password_enabled:boolean };
export function GithubButton({bind=false,disabled=false,compact=false}: {bind?:boolean;disabled?:boolean;compact?:boolean}) {
  const t=useText();const [busy,setBusy]=useState(false);const [error,setError]=useState<string>();
  async function start(){setBusy(true);setError(undefined);try{const r=await post<{authorization_url:string}>('/github/start',{bind});const url=new URL(r.authorization_url);if(url.origin!=='https://github.com'||url.pathname!=='/login/oauth/authorize')throw new Error();window.location.assign(url.href);}catch(e){setError(e instanceof ApiError?e.code:'github_unavailable');setBusy(false);}}
  return <Space orientation="vertical" style={compact?undefined:{width:'100%'}}><Button block={!compact} disabled={disabled} loading={busy} onClick={()=>void start()}>{t(bind?'githubLink':'githubLogin')}</Button>{error&&<Alert type="error" showIcon title={t(error)}/>}</Space>;
}
export function GithubAccount({onStatus,compact=false}:{onStatus:(status:GithubStatus)=>void;compact?:boolean}) {
  const t=useText();const [status,setStatus]=useState<GithubStatus>();const [busy,setBusy]=useState(false);const [error,setError]=useState<string>();
  async function refresh(){const result=await api<GithubStatus>('/github');setStatus(result);onStatus(result);}
  useEffect(()=>{void refresh().catch(e=>setError(e instanceof ApiError?e.code:'networkError'));},[]);
  async function unlink(){setBusy(true);setError(undefined);try{await api('/github',{method:'DELETE'});await refresh();}catch(e){setError(e instanceof ApiError?e.code:'networkError');}finally{setBusy(false);}}
  if(!status?.enabled&&!status?.login)return error?<Alert type="error" title={t(error)}/>:null;
  const content=<Space orientation={compact?'horizontal':'vertical'} wrap={compact} style={compact?undefined:{width:'100%'}}>
    {status?.login?<><Typography.Text>{status.login}</Typography.Text><Button loading={busy} disabled={!status.password_enabled} onClick={()=>void unlink()}>{t('githubUnlink')}</Button>{!status.password_enabled&&<Typography.Text type="secondary">{t('last_login_method')}</Typography.Text>}</>:<GithubButton bind compact={compact}/>}
    {error&&<Alert type="error" title={t(error)} showIcon/>}
  </Space>;
  return compact?<div className="account-github"><span className="account-label">GitHub</span>{content}</div>:<Card title="GitHub">{content}</Card>;
}
