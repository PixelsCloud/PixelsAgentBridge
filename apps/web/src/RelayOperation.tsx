import { useEffect, useState } from 'react';
import { Alert, Button, Descriptions, Space, Tag, Typography } from 'antd';
import { useResource } from './useResource';
import { useDate, useText } from './i18n';
import { remainingSeconds, type RelayOperation } from './relayOperationState';

export function RelayOperationPanel({ id, onConfirmed, onRetry, busy }: {
  id: string;
  onConfirmed: () => void;
  onRetry?: () => void;
  busy: boolean;
}) {
  const t = useText(); const date = useDate();
  // Query after submission settles: a lookup before its commit can return 404
  // and incorrectly show an unconfirmed outcome until the next polling cycle.
  const resource = useResource<RelayOperation>(busy ? null : `/relay-operations/${encodeURIComponent(id)}`);
  const [clock, setClock] = useState({ received: performance.now(), elapsed: 0 });
  const operation = resource.data;
  const expired = operation?.error_code === 'operator_lease_expired';
  const superseded = operation?.error_code === 'superseded';
  useEffect(() => {
    if (!operation) return;
    setClock({ received: performance.now(), elapsed: 0 });
    onConfirmed();
  }, [operation, onConfirmed]);
  useEffect(() => {
    if (operation && ['completed', 'failed'].includes(operation.phase) && !resource.error) return;
    const poll = () => { if (document.visibilityState !== 'hidden') resource.refresh(); };
    const timer = window.setInterval(poll, 5000);
    document.addEventListener('visibilitychange', poll);
    return () => { window.clearInterval(timer); document.removeEventListener('visibilitychange', poll); };
  }, [operation?.phase, resource.error, resource.refresh]);
  useEffect(() => {
    if (operation?.phase !== 'waiting') return;
    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'hidden') setClock(current => ({ ...current, elapsed: performance.now() - current.received }));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [operation?.phase]);
  return <Space orientation="vertical" style={{ width: '100%' }} data-testid="relay-operation">
    <Typography.Title level={5}>{t('relayOperation')}</Typography.Title>
    <Typography.Text copyable>{id}</Typography.Text>
    {resource.error && <Alert type="warning" showIcon title={t('relayUnconfirmed')} description={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    {operation && <>
      <Tag color={resource.error || expired || superseded ? 'default' : operation.phase === 'failed' ? 'error' : operation.phase === 'waiting' ? 'processing' : 'success'}>{t(superseded ? 'relayOperationSuperseded' : expired ? 'relayOperationExpired' : `relayPhase.${operation.phase}`)}</Tag>
      <Descriptions column={{ xs: 1, sm: 2 }} items={[
        { key: 'node', label: t('name'), children: operation.node_id },
        { key: 'action', label: t('action'), children: t(`relayAction.${operation.action}`) },
        { key: 'actor', label: t('actor'), children: operation.actor },
        { key: 'started', label: t('time'), children: date(operation.started_at_unix_ms) },
        { key: 'deadline', label: t('relayDeadline'), children: date(operation.deadline_unix_ms) },
        { key: 'countdown', label: t('relayCountdown'), children: resource.error ? t('unknown') : operation.phase === 'waiting' ? `${remainingSeconds(operation, clock.elapsed)} s` : '—' },
        { key: 'capacity', label: t('relayRemainingCapacity'), children: resource.error ? t('unknown') : operation.remaining_host_bandwidth ? t('relayHostBandwidth') : operation.remaining_capacity_mbps === null ? t('unknown') : `${operation.remaining_capacity_mbps} Mbps` },
        { key: 'sampled', label: t('relaySampledAt'), children: date(operation.capacity_sampled_at_unix_ms) },
        { key: 'completed', label: t('relayCompletedAt'), children: operation.completed_at_unix_ms === null ? '—' : date(operation.completed_at_unix_ms) },
        { key: 'outage', label: t('relayAllowOutage'), children: t(operation.allow_outage ? 'enabled' : 'disabled') },
      ]}/>
      {operation.phase === 'failed' && <Alert type={expired ? 'info' : 'error'} title={t(expired ? 'relayLeaseExpired' : 'relayOperationFailed')} description={expired ? t('relayExpiredRecordHint') : operation.error_code === 'node_confirmation_timeout' ? t('relayNodeConfirmationTimeout') : operation.error_code ?? t('unknown')}/>}
      {operation.phase === 'ready' && <Typography.Paragraph type="secondary">{t('relayReadyHint')}</Typography.Paragraph>}
    </>}
    {!operation && !resource.error && <Typography.Text>{t('loading')}</Typography.Text>}
    {onRetry && <Button loading={busy} onClick={onRetry}>{t('relayRetryOriginal')}</Button>}
    <Typography.Paragraph type="secondary">{t('relayProbeNotRun')}</Typography.Paragraph>
  </Space>;
}
