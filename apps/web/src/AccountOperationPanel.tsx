import { Alert, Button, Descriptions, Space, Typography } from 'antd';
import { useDate, useText } from './i18n';
import type { useAccountOperation } from './useAccountOperation';

export function AccountOperationPanel({ operation, onRetry, busy }: {
  operation: ReturnType<typeof useAccountOperation>; onRetry: () => void; busy: boolean;
}) {
  const t = useText(); const date = useDate();
  if (!operation.intent) return null;
  return <Space orientation="vertical" style={{ width: '100%' }} data-testid="account-operation">
    <Typography.Text copyable style={{ overflowWrap: 'anywhere' }}>{operation.intent.operation_id}</Typography.Text>
    {operation.result ? <>
      <Alert type="success" showIcon title={t('accountOperationCommitted')}/>
      <Descriptions column={{ xs: 1, sm: 2 }} items={[
        { key: 'kind', label: t('action'), children: t(operation.result.kind === 'password_reset' ? 'resetPassword' : 'revokeSessions') },
        { key: 'count', label: t('sessionsRevoked'), children: operation.result.sessions_revoked },
        { key: 'time', label: t('time'), children: date(operation.result.completed_at) },
      ]}/>
      {operation.result.propagation === 'pending' && <Typography.Text type="secondary">{t('sessionPropagationPending')}</Typography.Text>}
    </> : <Alert type="warning" title={t('accountOperationUnconfirmed')} description={operation.error ? t(operation.error) : t('loading')} action={<Button onClick={operation.refresh}>{t('refresh')}</Button>}/>}
    {operation.pending && <Button onClick={onRetry} loading={busy}>{t('retryOriginalOperation')}</Button>}
  </Space>;
}
