import { useEffect, useState } from 'react';
import { Alert, App, Button, Card, Checkbox, Descriptions, Form, Input, Modal, Space, Tag, Typography } from 'antd';
import { useNavigate, useParams } from 'react-router-dom';
import { api, ApiError, type Viewer } from './api';
import { useLiveRefresh, useResource } from './useResource';
import { PageHeading } from './PageHeading';
import { useDate, useText } from './i18n';
import { SessionsPanel } from './SessionsPanel';
import { AccountOperationPanel } from './AccountOperationPanel';
import { useAccountOperation, type AccountIntent, type AccountOperation } from './useAccountOperation';

interface AccountDetail {
  id: string; username: string; status: string; server_admin: boolean; revision: number;
  created_at: number; relay_limit_mbps: number; password_enabled: boolean; github_enabled: boolean; device_count: number;
  usage: { from: number; until: number; counters: Record<string, string>; incomplete: boolean; updated_at: number | null };
}
type ResetIntent = Extract<AccountIntent, { kind: 'password_reset' }>;

export function AccountDetailPage({ me, liveRevision }: { me: Viewer; liveRevision: number }) {
  const { id = '' } = useParams();
  return <AccountDetailContent key={id} id={id} me={me} liveRevision={liveRevision}/>;
}

function AccountDetailContent({ id, me, liveRevision }: { id: string; me: Viewer; liveRevision: number }) {
  const navigate = useNavigate(); const t = useText(); const date = useDate(); const { message } = App.useApp();
  const resource = useResource<AccountDetail>(`/accounts/${encodeURIComponent(id)}`);
  const operation = useAccountOperation(`pab-account-reset:${me.id}:${id}`);
  const [intent, setIntent] = useState<ResetIntent>(); const [busy, setBusy] = useState(false); const [form] = Form.useForm();
  useLiveRefresh(resource.refresh, liveRevision);
  useEffect(() => { if (operation.result) resource.refresh(); }, [operation.result, resource.refresh]);
  const account = resource.data;
  const openReset = (retry?: ResetIntent) => {
    form.resetFields();
    const value = retry ?? { kind: 'password_reset' as const, operation_id: crypto.randomUUID(), revoke_sessions: true };
    form.setFieldsValue({ revoke_sessions: value.revoke_sessions }); setIntent(value);
  };
  async function reset(values: { new_password: string; revoke_sessions: boolean }) {
    if (!account || !intent || busy) return;
    const request = { ...intent, revoke_sessions: operation.pending ? intent.revoke_sessions : values.revoke_sessions };
    setBusy(true); operation.begin(request);
    try {
      const result = await api<AccountOperation>(`/accounts/${id}/password-reset`, { method: 'POST', body: JSON.stringify({ operation_id: request.operation_id, revision: account.revision, new_password: values.new_password, revoke_sessions: request.revoke_sessions }) });
      operation.accept(result); resource.refresh(); message.success(t('accountOperationCommitted'));
    } catch (error) { operation.reject(error); if (error instanceof ApiError && error.status < 500) { message.error(t(error.code)); resource.refresh(); } }
    finally { form.resetFields(); setIntent(undefined); setBusy(false); }
  }
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <PageHeading title={account?.username ?? t('accountDetails')} description={t('accountPageHint')} actions={<Button onClick={() => navigate('/accounts')}>{t('back')}</Button>}/>
    {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    <Card title={t('accountDetails')} loading={resource.loading}>{account && <>
      <Descriptions column={{ xs: 1, sm: 2 }} items={[
        { key: 'name', label: t('username'), children: account.username },
        { key: 'role', label: t('role'), children: t(account.server_admin ? 'admin' : 'user') },
        { key: 'status', label: t('accountStatus'), children: <Tag>{t(account.status === 'active' ? 'enabled' : 'disabled')}</Tag> },
        { key: 'created', label: t('created'), children: date(account.created_at) },
        { key: 'bandwidth', label: t('userBandwidth'), children: `${account.relay_limit_mbps} Mbps` },
        { key: 'login', label: t('loginMethods'), children: [account.password_enabled ? t('localPassword') : '', account.github_enabled ? 'GitHub' : ''].filter(Boolean).join(' / ') || t('unknown') },
        { key: 'devices', label: t('associatedDevices'), children: account.device_count },
      ]}/>
      <Typography.Paragraph type="secondary">{t('accountDeviceAssociationHint')}</Typography.Paragraph>
      <Space wrap><Button onClick={() => navigate(`/devices?user=${account.id}`)}>{t('associatedDevices')}</Button><Button onClick={() => navigate(`/usage?user=${account.id}&period=7&from=${account.usage.from}&until=${account.usage.until}`)}>{t('viewAccountUsage')}</Button></Space>
    </>}</Card>
    {account && <>
      <Card title={t('recentAccountUsage')}>
        <Typography.Paragraph type="secondary">{date(account.usage.from)} — {date(account.usage.until)} ({Intl.DateTimeFormat().resolvedOptions().timeZone})</Typography.Paragraph>
        <Typography.Paragraph type="secondary">{t('usageHint')}</Typography.Paragraph>
        {account.usage.incomplete && <Alert type="warning" title={t('usageIncomplete')}/>}
        <Descriptions column={{ xs: 1, sm: 2 }} items={['relay_upload_bytes', 'relay_download_bytes', 'uploaded_bytes', 'downloaded_bytes', 'connections'].map(key => ({ key, label: t(key), children: account.usage.updated_at === null ? '—' : `${account.usage.counters[key] ?? '0'}${key.endsWith('_bytes') ? ' B' : ''}` }))}/>
      </Card>
      <Card title={t('resetPassword')}>
        {!account.password_enabled && <Alert type="info" title={t('password_login_required')}/>}
        {me.id === id ? <Button onClick={() => navigate('/account')}>{t('changePassword')}</Button> : <Button danger disabled={!account.password_enabled || !!resource.error || operation.pending || busy} onClick={() => openReset()}>{t('resetPassword')}</Button>}
        <AccountOperationPanel operation={operation} busy={busy} onRetry={() => { if (operation.intent?.kind === 'password_reset') openReset(operation.intent); }}/>
      </Card>
      <SessionsPanel key={id} viewerId={me.id} accountId={id} liveRevision={liveRevision}/>
    </>}
    <Modal title={`${t('resetPassword')} · ${account?.username ?? ''}`} open={!!intent} maskClosable={false} keyboard={false} onCancel={() => { if (!busy) { form.resetFields(); setIntent(undefined); } }} onOk={() => form.submit()} confirmLoading={busy} okText={t('confirm')} okButtonProps={{ 'aria-label': t('confirm') }} cancelText={t('cancel')}>
      <Typography.Paragraph>{t('passwordResetHint')}</Typography.Paragraph>
      {operation.pending && <Alert type="warning" title={t('accountRetryPasswordHint')}/>}
      <Form form={form} layout="vertical" disabled={busy} onFinish={reset} onValuesChange={() => { if (!operation.pending && !busy) setIntent(current => current ? { ...current, operation_id: crypto.randomUUID() } : current); }}>
        <Form.Item name="new_password" label={t('newPassword')} rules={[{ required: true, message: t('required') }, { min: 8, message: t('passwordMin') }]}><Input.Password autoComplete="new-password" maxLength={1024}/></Form.Item>
        <Form.Item name="confirm" label={t('confirmPassword')} dependencies={['new_password']} rules={[{ required: true, message: t('required') }, ({ getFieldValue }) => ({ validator: (_, value) => value === getFieldValue('new_password') ? Promise.resolve() : Promise.reject(new Error(t('passwordMismatch'))) })]}><Input.Password autoComplete="new-password" maxLength={1024}/></Form.Item>
        <Form.Item name="revoke_sessions" valuePropName="checked"><Checkbox disabled={operation.pending || busy}>{t('resetRevokeSessions')}</Checkbox></Form.Item>
      </Form>
    </Modal>
  </Space>;
}
