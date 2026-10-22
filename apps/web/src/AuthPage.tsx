import { useRef, useState } from 'react';
import { Alert, Button, Card, Form, Input, Space, Typography } from 'antd';
import { ApiError, post, type Viewer } from './api';
import { useText } from './i18n';
import brand from './brand.svg';
import { GithubButton } from './GithubAccount';
import { ArrowRight } from 'lucide-react';

export function AuthPage({ registration, github, onLogin }: { registration: boolean; github?:boolean; onLogin: (me: Viewer) => void }) {
  const t = useText();
  const [register, setRegister] = useState(false);
  const [busy, setBusy] = useState(false);
  const submitting = useRef(false);
  const [error, setError] = useState<string|undefined>(()=>new URLSearchParams(window.location.search).get('github_error')??undefined);
  const [form] = Form.useForm();
  async function submit(values: { username: string; password: string }) {
    if (submitting.current) return;
    submitting.current = true;
    setBusy(true); setError(undefined);
    try { onLogin(await post<Viewer>(register ? '/register' : '/session', { username: values.username, password: values.password })); }
    catch (e) { setError(e instanceof ApiError ? e.code : 'networkError'); }
    finally { submitting.current = false; setBusy(false); }
  }
  return <div className="auth-wrap"><div className="auth-intro">
    <div className="auth-brand"><img className="brand-mark" src={brand} alt=""/><strong><span className="brand-wordmark">Pixels</span><span className="brand-subtitle">Agent Bridge</span></strong></div>
    <div className="auth-story"><Typography.Title>{t('authTitle')}</Typography.Title></div>
  </div><div className="auth-form-area">
    <Card className="auth-card"><Space orientation="vertical" size={24} style={{ width: '100%' }}>
      <div><span className="auth-form-kicker">{t(register ? 'console' : 'authWelcome')}</span><Typography.Title level={2}>{t(register ? 'register' : 'login')}</Typography.Title><Typography.Text type="secondary">{t(register ? 'registerIntro' : 'authSecureHint')}</Typography.Text></div>
      {error && <Alert type="error" title={t(error)} showIcon/>}
      <Form form={form} layout="vertical" onFinish={submit} requiredMark={false}>
        <Form.Item name="username" label={t('username')} rules={[{ required: true, message: t('required') }, ...(register ? [{ min: 3, max: 64, message: t('usernameHint') }] : [])]}><Input autoComplete="username" size="large" maxLength={64}/></Form.Item>
        <Form.Item name="password" label={t('password')} extra={register ? t('passwordMin') : undefined} rules={[{ required: true, message: t('required') }, ...(register ? [{ min: 8, message: t('passwordMin') }] : [])]}><Input.Password autoComplete={register ? 'new-password' : 'current-password'} size="large" maxLength={1024}/></Form.Item>
        {register && <Form.Item name="confirm" label={t('confirmPassword')} dependencies={['password']} rules={[{ required: true, message: t('required') }, ({ getFieldValue }) => ({ validator: (_, value) => !value || value === getFieldValue('password') ? Promise.resolve() : Promise.reject(new Error(t('passwordMismatch'))) })]}><Input.Password autoComplete="new-password" size="large"/></Form.Item>}
        <Button className="auth-submit" type="primary" htmlType="submit" block size="large" disabled={busy} aria-busy={busy}>{t(busy ? register ? 'authRegistering' : 'authSigningIn' : register ? 'register' : 'login')}<ArrowRight size={17}/></Button>
      </Form>
      {registration && <Button type="link" disabled={busy} onClick={() => { setRegister(!register); setError(undefined); form.resetFields(); }}>{t(register ? 'alreadyAccount' : 'createAccount')}</Button>}
      {github && <GithubButton disabled={busy}/>}
    </Space></Card></div></div>;
}
