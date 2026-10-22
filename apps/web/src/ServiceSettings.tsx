import { ConsoleSelect } from './ConsoleSelect';
import { useEffect, useState } from 'react';
import { Alert, App, Button, Card, Descriptions, Space, Typography } from 'antd';
import { api, ApiError } from './api';
import { useText } from './i18n';
import { useLiveRefresh, useResource } from './useResource';

interface RegistrationPolicy {
  deployment_default: boolean; override_value: boolean | null; effective: boolean;
  source: 'runtime_override' | 'deployment_default'; revision: number; effect: 'immediate';
}
interface Settings { registration_enabled: boolean; policy_revision: number; registration?: RegistrationPolicy }
type Mode = 'default' | 'enabled' | 'disabled';
const modeOf = (policy: RegistrationPolicy): Mode => policy.override_value === null ? 'default' : policy.override_value ? 'enabled' : 'disabled';

export function ServiceSettings({ liveRevision = 0 }: { liveRevision?: number }) {
  const t = useText(); const { message } = App.useApp(); const resource = useResource<Settings>('/service');
  const [draft, setDraft] = useState<{ mode: Mode; revision: number }>();
  const [committed, setCommitted] = useState<RegistrationPolicy>();
  const [busy, setBusy] = useState(false); const [error, setError] = useState<string>();
  useLiveRefresh(resource.refresh, liveRevision);
  const loaded = resource.data?.registration;
  const policy = committed && (!loaded || loaded.revision < committed.revision) ? committed : loaded;
  const stale = !!draft && !!policy && draft.revision !== policy.revision;
  async function save(mode: Mode) {
    if (!policy || stale) return;
    setBusy(true); setError(undefined);
    try {
      const result = await api<{ registration: RegistrationPolicy }>('/service', { method: 'PATCH', body: JSON.stringify({ revision: draft?.revision ?? policy.revision, registration_override: mode === 'default' ? null : mode === 'enabled' }) });
      setCommitted(result.registration); setDraft(undefined); message.success(t('policyCommitted')); resource.refresh();
    } catch (reason) {
      setError(reason instanceof ApiError && reason.status < 500 ? reason.code : 'policyUnconfirmed'); resource.refresh();
    } finally { setBusy(false); }
  }
  const refresh = () => { setDraft(undefined); setCommitted(undefined); setError(undefined); resource.refresh(); };
  return <Card title={t('service')} loading={resource.loading && !resource.data}>
    <Space orientation="vertical" size={16} style={{ width: '100%' }}>
      {resource.error && <Alert type="error" title={t(resource.error)}/>}
      {error && <Alert type="warning" title={t(error)}/>}
      {stale && <Alert type="warning" title={t('policyStale')}/>}
      <Typography.Paragraph type="secondary">{t('policyHint')}</Typography.Paragraph>
      {resource.data && <Descriptions column={{ xs: 1, lg: 2 }} items={[
        { key: 'effective', label: t('policyEffective'), children: policy ? t(policy.effective ? 'enabled' : 'disabled') : t(resource.data.registration_enabled ? 'enabled' : 'disabled') },
        { key: 'default', label: t('policyDefault'), children: policy ? t(policy.deployment_default ? 'enabled' : 'disabled') : t('unknown') },
        { key: 'override', label: t('policyOverride'), children: policy ? t(policy.override_value === null ? 'policyInherited' : policy.override_value ? 'enabled' : 'disabled') : t('unknown') },
        { key: 'source', label: t('policySource'), children: policy ? t(`policySource.${policy.source}`) : t('unknown') },
        { key: 'revision', label: t('policyRevision'), children: policy?.revision ?? t('unknown') },
        { key: 'effect', label: t('policyEffect'), children: policy ? t('policyImmediate') : t('unknown') },
        { key: 'policy', label: t('policyOffered'), children: resource.data.policy_revision },
        { key: 'session', label: t('sessionDuration'), children: t('permanentSession') },
      ]}/>}
      <Space wrap>
        <ConsoleSelect aria-label={t('policyOverride')} style={{ width: 220, maxWidth: '100%' }} disabled={!policy || busy || !!resource.error} value={draft?.mode ?? (policy ? modeOf(policy) : undefined)} options={[
          { value: 'default', label: t('policyInherited') }, { value: 'enabled', label: t('enabled') }, { value: 'disabled', label: t('disabled') },
        ]} onChange={(mode: Mode) => policy && setDraft({ mode, revision: draft?.revision ?? policy.revision })}/>
        <Button type="primary" aria-label={t('save')} aria-busy={busy} loading={busy} disabled={!draft || stale || !!resource.error || !policy || draft.mode === modeOf(policy)} onClick={() => draft && void save(draft.mode)}>{t('save')}</Button>
        <Button disabled={!policy || policy.override_value === null || stale || busy || !!resource.error} onClick={() => void save('default')}>{t('policyRestore')}</Button>
        <Button disabled={busy} onClick={refresh}>{t('refresh')}</Button>
      </Space>
    </Space>
  </Card>;
}
