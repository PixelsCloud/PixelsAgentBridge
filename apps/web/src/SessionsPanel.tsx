import { useEffect, useState } from 'react';
import { paginationComponents } from './ConsoleSelect';
import { Alert, App, Button, Card, Modal, Space, Table, Tag, Tooltip, Typography } from 'antd';
import { Info, ShieldCheck } from 'lucide-react';
import { api, ApiError } from './api';
import { useDate, useText } from './i18n';
import { usePage } from './usePagedResource';
import { AccountOperationPanel } from './AccountOperationPanel';
import { useAccountOperation, type AccountIntent, type AccountOperation } from './useAccountOperation';

interface LoginSession { id: string; channel: string; created_at: number; last_active_at: number | null; current: boolean }
type SessionIntent = Extract<AccountIntent, { kind: 'session_revoke' }>;

export function SessionsPanel({ viewerId, accountId, liveRevision, own = false, compact = false }: {
  viewerId: string; accountId: string; liveRevision: number; own?: boolean; compact?: boolean;
}) {
  const t = useText(); const date = useDate(); const { message } = App.useApp();
  const path = own ? '/sessions' : `/accounts/${accountId}/sessions`;
  const resource = usePage<LoginSession>(path, liveRevision, compact ? 5 : undefined);
  const operation = useAccountOperation(`pab-account-sessions:${viewerId}:${accountId}`);
  const [intent, setIntent] = useState<SessionIntent>(); const [busy, setBusy] = useState(false);
  useEffect(() => { if (operation.result) resource.refresh(); }, [operation.result, resource.refresh]);
  const choose = (scope: SessionIntent['scope'], session_id?: string) => setIntent({ kind: 'session_revoke', operation_id: crypto.randomUUID(), scope, ...(session_id ? { session_id } : {}) });
  async function submit(value: SessionIntent) {
    if (busy) return; setBusy(true); operation.begin(value);
    try {
      const result = await api<AccountOperation>(`${path}/revoke`, { method: 'POST', body: JSON.stringify({ operation_id: value.operation_id, scope: value.scope, session_id: value.session_id }) });
      operation.accept(result); resource.refresh(); message.success(t('accountOperationCommitted'));
    } catch (error) { operation.reject(error); if (error instanceof ApiError && error.status < 500) message.error(t(error.code)); }
    finally { setBusy(false); setIntent(undefined); }
  }
  return <Card className={compact ? 'account-sessions' : undefined} title={compact ? <span className="account-section-title"><ShieldCheck size={17}/>{t('loginSessions')}</span> : t('loginSessions')}>
    <Typography.Paragraph className={compact ? 'account-session-hint' : undefined} type="secondary">{t('permanentSession')}{compact ? <Tooltip title={t('sessionMetadataHint')}><Info size={14} tabIndex={0} aria-label={t('sessionMetadataHint')}/></Tooltip> : ` · ${t('sessionMetadataHint')}`}</Typography.Paragraph>
    <Space className={compact ? 'account-session-actions' : undefined} wrap style={compact ? undefined : { marginBottom: 16 }}>
      <Button onClick={resource.refresh}>{t('refresh')}</Button>
      {viewerId === accountId && <Button disabled={busy || operation.pending} onClick={() => choose('others')}>{t('revokeOtherSessions')}</Button>}
      <Button danger disabled={busy || operation.pending || !resource.data?.total} onClick={() => choose('all')}>{t('revokeAllSessions')}</Button>
    </Space>
    {resource.error && <Alert type="error" title={t(resource.error)}/>}
    <Table<LoginSession> size={compact ? 'small' : undefined} rowKey="id" dataSource={resource.data?.items ?? []} loading={resource.loading} scroll={{ x: compact ? 590 : 650 }} pagination={{ components: paginationComponents, current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? (compact ? 5 : 20), total: resource.data?.total ?? 0, onChange: resource.paginate }} columns={[
      { title: t('sessionSource'), render: (_, row) => <div><Space size={4} wrap><Tag>{t(`sessionChannel.${row.channel}`)}</Tag>{row.current && <Tag color="processing">{t('currentSession')}</Tag>}</Space>{compact && <Typography.Text className="account-session-id" copyable={{ text: row.id }} title={row.id}>{row.id.slice(0, 8)}…{row.id.slice(-4)}</Typography.Text>}</div> },
      { title: t('created'), render: (_, row) => date(row.created_at) },
      { title: t('sessionLastActive'), render: (_, row) => row.last_active_at === null ? t('unknown') : date(row.last_active_at) },
      ...(!compact ? [{ title: t('sessionId'), render: (_: unknown, row: LoginSession) => <Typography.Text copyable>{row.id}</Typography.Text> }] : []),
      { title: t('actions'), render: (_, row) => <Button danger size="small" disabled={busy || operation.pending} onClick={() => choose('session', row.id)}>{t('revokeSession')}</Button> },
    ]}/>
    <AccountOperationPanel operation={operation} busy={busy} onRetry={() => { if (operation.intent?.kind === 'session_revoke') void submit(operation.intent); }}/>
    <Modal title={t('revokeSessions')} open={!!intent} onCancel={() => !busy && setIntent(undefined)} maskClosable={false} keyboard={false} onOk={() => intent && void submit(intent)} confirmLoading={busy} okText={t('confirm')} okButtonProps={{ 'aria-label': t('confirm') }} cancelText={t('cancel')}>
      <Typography.Paragraph>{t('sessionRevokeHint')}</Typography.Paragraph>
      <Typography.Text strong>{t(intent?.scope === 'all' ? 'revokeAllSessions' : intent?.scope === 'others' ? 'revokeOtherSessions' : 'revokeSession')}</Typography.Text>
      {intent?.session_id && <Typography.Paragraph code>{intent.session_id}</Typography.Paragraph>}
      {viewerId === accountId && intent?.scope !== 'others' && <Alert type="warning" title={t('sessionMaySignOut')}/>}
    </Modal>
  </Card>;
}
