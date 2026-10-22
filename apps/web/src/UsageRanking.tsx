import { useContext } from 'react';
import { Alert, Button, Card, Space, Table, Typography } from 'antd';
import { useSearchParams } from 'react-router-dom';
import { LanguageContext, useDate, useText } from './i18n';
import { paginationComponents } from './ConsoleSelect';
import { useLiveRefresh, useResource } from './useResource';
import { counter, metrics, usageValue, usageExactValue, type Counters } from './usageModel';

interface Ranking { items: { subject: string; user_id: string | null; username: string | null; counters_exact: Counters; incomplete: boolean; p2p_reported?: boolean; updated_at: number | null }[]; total: number; page: number; page_size: number; from: number | null; until: number | null; interval: string }

export function UsageRanking({ query, liveRevision, disabled }: { query: URLSearchParams; liveRevision: number; disabled: boolean }) {
  const t = useText(); const date = useDate(); const [params, setParams] = useSearchParams();
  const language = useContext(LanguageContext);
  const sort = params.get('sort') ?? 'relay_download_bytes'; const order = params.get('order') ?? 'desc';
  const ranking = new URLSearchParams(query); ranking.set('page', params.get('rank_page') ?? '1'); ranking.set('page_size', params.get('rank_size') ?? '20'); ranking.set('sort', sort); ranking.set('order', order);
  const resource = useResource<Ranking>(disabled ? null : `/usage/ranking?${ranking}`);
  useLiveRefresh(resource.refresh, liveRevision);
  const update = (key: string, value: string) => { const next = new URLSearchParams(params); next.set(key, value); next.delete('rank_page'); setParams(next); };
  const selected = (id: string) => {
    const next = new URLSearchParams(params); next.set('user', id); next.delete('rank_page');
    if (resource.data?.from != null && resource.data.until != null) { next.set('from', String(resource.data.from)); next.set('until', String(resource.data.until)); }
    setParams(next);
  };
  return <Card title={t('usageRanking')}>
    <Space wrap style={{ marginBottom: 16 }}>
      <select className="console-select" aria-label={t('usageSort')} value={sort} onChange={event => update('sort', event.target.value)}>{metrics.map(value => <option key={value} value={value}>{t(value)}</option>)}</select>
      <select className="console-select" aria-label={t('usageOrder')} value={order} onChange={event => update('order', event.target.value)}>{['desc', 'asc'].map(value => <option key={value} value={value}>{t(`usageOrder.${value}`)}</option>)}</select>
    </Space>
    {resource.error && <Alert type="error" title={t(resource.error)}/>}
    <Table rowKey="subject" loading={resource.loading} dataSource={resource.error ? [] : resource.data?.items ?? []} scroll={{ x: 650 }} pagination={{ components: paginationComponents, current: resource.data?.page ?? 1, pageSize: resource.data?.page_size ?? 20, total: resource.data?.total ?? 0, onChange: (page, size) => { const next = new URLSearchParams(params); next.set('rank_page', String(page)); next.set('rank_size', String(size)); setParams(next); } }} columns={[
      { title: t('username'), render: (_, row) => row.user_id ? <Button type="link" onClick={() => selected(row.user_id!)}>{row.username ?? row.subject}</Button> : <Typography.Text>{row.subject === 'guest' ? t('usagePopulation_guests') : row.subject}</Typography.Text> },
      { title: t(sort), render: (_, row) => { const value = counter(row.counters_exact, undefined, sort); return sort.startsWith('p2p_') && !row.p2p_reported ? t('p2pUnreported') : value === undefined ? t('unknown') : <span title={usageExactValue(sort, value, language)}>{usageValue(sort, value, language)}</span>; } },
      { title: t('lastSeen'), render: (_, row) => row.updated_at === null ? t('unknown') : date(row.updated_at) },
    ]}/>
  </Card>;
}
