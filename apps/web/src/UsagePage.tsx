import { paginationComponents } from './ConsoleSelect';
import { useContext, useEffect, useState } from 'react';
import { Alert, Button, Card, Input, Space, Table, Typography } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { ApiError } from './api';
import { LanguageContext, useDate, useText } from './i18n';
import { PageHeading } from './PageHeading';
import { useLiveRefresh, useResource } from './useResource';
import { UsageRanking } from './UsageRanking';
import { UsageChart } from './UsageChart';
import { counter, localDateTime, metrics, parseLocalDateTime, usageExactValue, usageValue, usageQuery, usageWindow, type UsageResult } from './usageModel';

export function UsagePage({ all, liveRevision }: { all: boolean; liveRevision: number }) {
  const t = useText(); const date = useDate(); const [params, setParams] = useSearchParams();
  const language = useContext(LanguageContext);
  const period = params.get('period') ?? (all ? '7' : 'today'); const population = params.get('population') ?? 'all';
  const rangeParams = new URLSearchParams(params); rangeParams.set('period', period);
  const window = usageWindow(rangeParams); const query = usageQuery(params, window, all);
  const path = window.error ? null : `/usage?${query}`;
  const resource = useResource<UsageResult>(path, { retainPreviousData: true });
  const limits = useResource<{ user_mbps: number }>('/traffic');
  const previousWindow = { ...window, from: window.from - (window.until - window.from), until: window.from };
  const previousQuery = usageQuery(params, previousWindow, all);
  const previous = useResource<UsageResult>(window.error || window.interval === 'lifetime' ? null : `/usage?${previousQuery}`, { retainPreviousData: true });
  const [fromInput, setFromInput] = useState(''); const [untilInput, setUntilInput] = useState(''); const [inputError, setInputError] = useState<string>();
  const [exporting, setExporting] = useState(false); const [exportError, setExportError] = useState<string>(); const [truncated, setTruncated] = useState(false);
  const stamp = `${window.from}:${window.until}:${window.interval}`;
  useEffect(() => { if (!window.error && window.interval !== 'lifetime') { setFromInput(localDateTime(window.from)); setUntilInput(localDateTime(window.until)); } }, [stamp]);
  useLiveRefresh(() => { resource.refresh(); previous.refresh(); }, liveRevision);
  useEffect(() => { setTruncated(false); setExportError(undefined); }, [path]);
  const update = (key: string, value?: string, reset = false) => {
    const next = new URLSearchParams(params); value ? next.set(key, value) : next.delete(key); next.delete('rank_page');
    if (reset) { next.delete('from'); next.delete('until'); }
    if (key === 'population') next.delete('user');
    if (key === 'period' && value !== 'lifetime' && next.get('interval') === 'lifetime') next.set('interval', 'auto');
    setInputError(undefined); setParams(next);
  };
  const applyRange = () => {
    const from = parseLocalDateTime(fromInput); const until = parseLocalDateTime(untilInput);
    if (from === undefined || until === undefined || until <= from) { setInputError('invalid_input'); return; }
    const next = new URLSearchParams(params); next.set('period', 'custom'); next.set('from', String(from)); next.set('until', String(until)); next.delete('rank_page');
    if (next.get('interval') === 'lifetime') next.set('interval', 'auto');
    setInputError(undefined); setParams(next);
  };
  async function download() {
    setExporting(true); setExportError(undefined); setTruncated(false);
    try {
      const response = await fetch(`/api/web/usage/export?${query}`, { credentials: 'same-origin' });
      if (!response.ok) {
        const error = await response.json().catch(() => ({ code: 'server_error' }));
        if (response.status === 401) globalThis.dispatchEvent(new Event('pab-session-expired'));
        throw new ApiError(response.status, error.code ?? 'server_error');
      }
      const blob = await response.blob();
      const url = URL.createObjectURL(blob); const link = document.createElement('a');
      link.href = url; link.download = 'pixels-usage.csv'; document.body.append(link); link.click(); link.remove();
      globalThis.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setTruncated(response.headers.get('x-usage-truncated') === 'true');
    } catch (error) { setExportError(error instanceof ApiError ? error.code : 'networkError'); }
    finally { setExporting(false); }
  }
  const data = resource.error ? undefined : resource.data;
  // A previous-period response can finish before or after the current period.
  // Never compare totals from different windows while a selection is updating.
  const previousData = !previous.error && data?.from != null && data.until != null
    && previous.data?.until === data.from && previous.data.from === data.from - (data.until - data.from)
    ? previous.data : undefined;
  const actualFrom = data?.from ?? window.from; const actualUntil = data?.until ?? window.until;
  const mainError = inputError ?? window.error ?? resource.error;
  return <Space orientation="vertical" size={20} style={{ width: '100%' }}>
    <PageHeading title={t(all ? 'serviceUsage' : 'myUsage')} description={t('usagePageHint')}/>
    <Card>
      <div className="usage-filter-bar">
        <select className="console-select" aria-label={t('usagePeriod')} value={period} onChange={event => update('period', event.target.value, event.target.value !== 'custom')}>{['today', 'month', '7', '30', '90', '180', '730', 'custom', 'lifetime'].map(value => <option key={value} value={value}>{Number.isFinite(Number(value)) ? `${value} ${t('days')}` : t(value)}</option>)}</select>
        {period !== 'lifetime' && <select className="console-select" aria-label={t('usageInterval')} value={params.get('interval') ?? 'auto'} onChange={event => update('interval', event.target.value)}>{['auto', 'hour', 'day'].map(value => <option key={value} value={value}>{t(`usageInterval.${value}`)}</option>)}</select>}
        {all && <select className="console-select" aria-label={t('usagePopulation')} value={population} onChange={event => update('population', event.target.value)}>{['all', 'users', 'guests'].map(value => <option key={value} value={value}>{t(`usagePopulation_${value}`)}</option>)}</select>}
        <Button loading={resource.loading} onClick={() => { resource.refresh(); previous.refresh(); }}>{t('refresh')}</Button>
        <Button loading={exporting} disabled={!!mainError || resource.loading || !data} onClick={() => void download()}>{t('usageExport')}</Button>
      </div>
      {period === 'custom' && <Space wrap style={{ marginTop: 16 }}>
        <label>{t('usageFrom')}<Input type="datetime-local" aria-label={t('usageFrom')} step={3600} value={fromInput} onChange={event => setFromInput(event.target.value)} style={{ width: 230, maxWidth: '100%' }}/></label>
        <label>{t('usageUntil')}<Input type="datetime-local" aria-label={t('usageUntil')} step={3600} value={untilInput} onChange={event => setUntilInput(event.target.value)} style={{ width: 230, maxWidth: '100%' }}/></label>
        <Button onClick={applyRange}>{t('usageApply')}</Button>
      </Space>}
      <div className="usage-meta">
        {window.interval !== 'lifetime' && !window.error && <span data-testid="usage-window" title={`${new Date(actualFrom).toISOString()} ≤ t < ${new Date(actualUntil).toISOString()} · UTC`}>
          {t('usageEffectiveRange')}: {date(actualFrom)} — {date(actualUntil)} · {t(`usageInterval.${data?.interval ?? window.interval}`)}
        </span>}
        <span>{data?.updated_at ? `${t('lastSeen')}: ${date(data.updated_at)}` : t('usagePending')}</span>
        {!all && <span>{t('userBandwidth')}: {limits.data ? `${limits.data.user_mbps} Mbps` : '—'}</span>}
      </div>
    </Card>
    {mainError && <Alert type="error" title={t(mainError)}/>}
    {exportError && <Alert type="error" title={t('usageExportFailed')} description={t(exportError)}/>}
    {truncated && <Alert type="warning" title={t('usageExportTruncated')} showIcon/>}
    {window.interval !== 'lifetime' && !window.error && <UsageChart points={data?.points ?? []}/>}
    <div className="usage-metrics-grid" aria-busy={resource.loading}>{metrics.map(key => {
      const value = counter(data?.counters_exact, data?.counters, key); const before = counter(previousData?.counters_exact, previousData?.counters, key);
      return <Card key={key} className="usage-metric-card">
        <Typography.Paragraph type="secondary">{t(key)}</Typography.Paragraph>
        <Typography.Paragraph strong title={data?.updated_at ? value === undefined ? t('usagePrecisionUnavailable') : usageExactValue(key, value, language) : undefined} style={{ fontSize: 24, overflowWrap: 'anywhere' }}>{key.startsWith('p2p_') && !data?.p2p_reported ? t('p2pUnreported') : data?.updated_at && value !== undefined ? usageValue(key, value, language) : '—'}</Typography.Paragraph>
        {window.interval !== 'lifetime' && <Typography.Text type="secondary">{t('previousPeriod')}: {key.startsWith('p2p_') && !previousData?.p2p_reported ? '—' : previousData?.updated_at && before !== undefined ? usageValue(key, before, language) : '—'}</Typography.Text>}
      </Card>;
    })}</div>
    {window.interval !== 'lifetime' && <Card title={t('usageHistory')}><Table rowKey="hour_unix_ms" size="small" scroll={{ x: 650 }} dataSource={data?.points ?? []} pagination={{ components: paginationComponents, defaultPageSize: 24 }} columns={[
      { title: t('time'), render: (_, point) => date(point.hour_unix_ms) },
      ...['relay_upload_bytes', 'relay_download_bytes', 'p2p_upload_bytes', 'p2p_download_bytes', 'uploaded_bytes', 'downloaded_bytes'].map(key => ({ title: t(key), render: (_: unknown, point: UsageResult['points'][number]) => { const value = counter(point.counters_exact, point.counters, key); return key.startsWith('p2p_') && !point.p2p_reported ? '—' : value === undefined ? t('usagePrecisionUnavailable') : <span title={usageExactValue(key, value, language)}>{usageValue(key, value, language)}</span>; } })),
    ]}/></Card>}
    {all && <UsageRanking query={query} liveRevision={liveRevision} disabled={!!window.error}/>}
  </Space>;
}
