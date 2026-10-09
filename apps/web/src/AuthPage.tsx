import { useState } from 'react';
import { Alert, Button, Card, Form, Input, Space, Typography } from 'antd';
import { ApiError, post, type Viewer } from './api';
import { useText } from './i18n';
import brand from './brand.svg';

export function AuthPage({ registration, onLogin }: { registration: boolean; onLogin: (me: Viewer) => void }) {
  const t = useText();
  const [register, setRegister] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [form] = Form.useForm();
  async function submit(values: { username: string; password: string }) {
    setBusy(true); setError(undefined);
    try { onLogin(await post<Viewer>(register ? '/register' : '/session', { username: values.username, password: values.password })); }
    catch (e) { setError(e instanceof ApiError ? e.code : 'networkError'); }
    finally { setBusy(false); }
  }
  return <div className="auth-wrap"><div className="auth-intro"><img className="brand-mark" src={brand} alt=""/><Typography.Title>{t('product')}</Typography.Title><Typography.Paragraph>{t('loginIntro')}</Typography.Paragraph></div>
    <Card className="auth-card"><Space orientation="vertical" size={24} style={{ width: '100%' }}>
      <div><Typography.Title level={2}>{t(register ? 'register' : 'login')}</Typography.Title><Typography.Text type="secondary">{t(register ? 'registerIntro' : 'loginIntro')}</Typography.Text></div>
      {error && <Alert type="error" title={t(error)} showIcon/>}
      <Form form={form} layout="vertical" onFinish={submit} requiredMark={false}>
        <Form.Item name="username" label={t('username')} rules={[{ required: true, message: t('required') }, ...(register ? [{ min: 3, max: 64, message: t('usernameHint') }] : [])]}><Input autoComplete="username" size="large" maxLength={64}/></Form.Item>
        <Form.Item name="password" label={t('password')} extra={register ? t('passwordMin') : undefined} rules={[{ required: true, message: t('required') }, ...(register ? [{ min: 8, message: t('passwordMin') }] : [])]}><Input.Password autoComplete={register ? 'new-password' : 'current-password'} size="large" maxLength={1024}/></Form.Item>
        {register && <Form.Item name="confirm" label={t('confirmPassword')} dependencies={['password']} rules={[{ required: true, message: t('required') }, ({ getFieldValue }) => ({ validator: (_, value) => !value || value === getFieldValue('password') ? Promise.resolve() : Promise.reject(new Error(t('passwordMismatch'))) })]}><Input.Password autoComplete="new-password" size="large"/></Form.Item>}
        <Button type="primary" htmlType="submit" block size="large" loading={busy}>{t(register ? 'register' : 'login')}</Button>
      </Form>
      {registration && <Button type="link" disabled={busy} onClick={() => { setRegister(!register); setError(undefined); form.resetFields(); }}>{t(register ? 'alreadyAccount' : 'createAccount')}</Button>}
    </Space></Card></div>;
}
