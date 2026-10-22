import { useState } from 'react';
import { Alert, App, Button, Card, Form, Input, Tag } from 'antd';
import { KeyRound, UserRound } from 'lucide-react';
import { ApiError, post, type Viewer } from './api';
import { GithubAccount, type GithubStatus } from './GithubAccount';
import { useText } from './i18n';
import { PageHeading } from './PageHeading';
import { SessionsPanel } from './SessionsPanel';
import './account.css';

export function AccountPage({ me }: { me: Viewer }) {
  const [github, setGithub] = useState<GithubStatus>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const t = useText();
  const { message } = App.useApp();

  async function submit(values: { current_password: string; new_password: string }) {
    setBusy(true); setError(undefined);
    try {
      await post('/password', { current_password: values.current_password, new_password: values.new_password });
      message.success(t('passwordChanged'));
    } catch (failure) {
      setError(failure instanceof ApiError ? failure.code : 'networkError');
    } finally { setBusy(false); }
  }

  return <div className="account-page">
    <PageHeading title={t('account')} description={t('accountPageHint')}/>
    <Card className="account-summary">
      <div className="account-identity">
        <span className="account-avatar" aria-hidden="true"><UserRound size={22}/></span>
        <div><span className="account-label">{t('username')}</span><div className="account-identity-line"><strong title={me.username}>{me.username}</strong><Tag>{t(me.server_admin ? 'admin' : 'user')}</Tag></div></div>
      </div>
      <GithubAccount onStatus={setGithub} compact/>
    </Card>
    <div className={`account-security-grid${github?.password_enabled ? '' : ' account-sessions-only'}`}>
      {github?.password_enabled && <Card className="account-password" title={<span className="account-section-title"><KeyRound size={17}/>{t('changePassword')}</span>}>
        <Form layout="vertical" onFinish={submit} requiredMark={false}>
          {error && <Alert className="account-password-error" type="error" title={t(error)} showIcon/>}
          <Form.Item name="current_password" label={t('currentPassword')} rules={[{ required: true, message: t('required') }]}><Input.Password autoComplete="current-password" maxLength={1024}/></Form.Item>
          <Form.Item name="new_password" label={t('newPassword')} extra={t('passwordMin')} rules={[{ required: true, message: t('required') }, { min: 8, message: t('passwordMin') }]}><Input.Password autoComplete="new-password" maxLength={1024}/></Form.Item>
          <Form.Item name="confirm" label={t('confirmPassword')} dependencies={['new_password']} rules={[{ required: true, message: t('required') }, ({ getFieldValue }) => ({ validator: (_, value) => value === getFieldValue('new_password') ? Promise.resolve() : Promise.reject(new Error(t('passwordMismatch'))) })]}><Input.Password autoComplete="new-password"/></Form.Item>
          <div className="account-password-actions"><Button type="primary" htmlType="submit" loading={busy}>{t('save')}</Button></div>
        </Form>
      </Card>}
      <SessionsPanel viewerId={me.id} accountId={me.id} liveRevision={0} own compact/>
    </div>
  </div>;
}
