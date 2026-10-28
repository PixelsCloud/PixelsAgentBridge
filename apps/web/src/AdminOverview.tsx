import { useContext, useState } from 'react';
import { Alert, Button } from 'antd';
import { Activity, Folder, RefreshCw } from 'lucide-react';
import { LanguageContext, useDate, useText } from './i18n';
import { byteText, durationText, type UsageResult } from './usageModel';
import { addValues, compactDuration, exactValue, percentChange, previousMatches, traffic, type Period, type RankingMetric, type Series } from './overviewModel';
import { useOverviewText } from './overviewText';
import { useDebouncedSearch, useOverviewData } from './useOverviewData';
import { OverviewFleet, OverviewPlatform, OverviewRelay } from './OverviewPanels';
import { OverviewDevices, OverviewRanking } from './OverviewTables';
import { OverviewTrend } from './OverviewTrend';
import './overview.css';

export function AdminOverview({ liveRevision }: { liveRevision: number }) {
  const text = useOverviewText(); const common = useText(); const date = useDate(); const language = useContext(LanguageContext);
  const [period, setPeriod] = useState<Period>('7'); const [series, setSeries] = useState<Series>('relay');
  const [sort, setSort] = useState<RankingMetric>('relay_download_bytes');
  const [page, setPage] = useState(1); const [search, setSearch] = useState(''); const [attention, setAttention] = useState(false);
  const query = useDebouncedSearch(search);
  const resources = useOverviewData(period, sort, page, query, attention, liveRevision);
  const summary = resources.overview.error ? undefined : resources.overview.data;
  const diagnostics = resources.diagnostics.error ? undefined : resources.diagnostics.data;
  const data = resources.usage.error ? undefined : resources.usage.data;
  const previous = previousMatches(data, resources.previous.error ? undefined : resources.previous.data);
  const trend = period === 'lifetime' ? resources.month.error ? undefined : resources.month.data : data;
  const errors = [...new Set([resources.overview.error, resources.diagnostics.error, resources.usage.error, resources.month.error, resources.ranking.error, resources.devices.error, resources.nodes.error].filter((value): value is string => !!value))];
  const usageLink = `/usage?period=${period}`;
  const amount = (value: bigint | undefined, kind: 'bytes' | 'duration' | 'count') => value === undefined ? '—' : kind === 'bytes' ? byteText(String(value)) : kind === 'duration' ? compactDuration(value, language) : value.toLocaleString(language);
  const raw = (value: bigint | undefined, duration = false) => value === undefined ? undefined : duration ? durationText(String(value), language) : `${value} B`;
  const comparison = (value: bigint | undefined, before: bigint | undefined) => {
    const change = percentChange(value, before);
    return period === 'lifetime' ? text('sinceStart') : change === undefined ? text('selectedPeriod') : `${text('previous')} ${change}`;
  };
  const bytes = (source: UsageResult | undefined, key: string) => amount(key.startsWith('p2p_') && !source?.p2p_reported ? undefined : exactValue(source, key), 'bytes');
  const totalFiles = addValues(exactValue(data, 'uploaded_files'), exactValue(data, 'downloaded_files'));
  const range = data?.from != null && data.until != null ? `${date(data.from)} — ${date(data.until)} · ${Intl.DateTimeFormat().resolvedOptions().timeZone}` : text('sinceStart');
  return <div className="admin-overview">
    <header className="page-heading"><div><div className="eyebrow">OPERATIONS / OVERVIEW</div><h2>{text('title')}<span>.</span></h2><p>{text('description')}</p></div><div className="heading-controls"><span className="capture"><span className="capture-dot"/>{text('updated')} <time>{summary?.relay_status?.sampled_at ? date(summary.relay_status.sampled_at) : '—'}</time></span><div className="segments period-tabs" role="group" aria-label={common('usagePeriod')}>{(['today', '7', 'month', 'lifetime'] as const).map(value => <button key={value} aria-pressed={period === value} onClick={() => setPeriod(value)}>{text(value === '7' ? 'week' : value)}</button>)}</div><Button className="overview-refresh" size="small" aria-label={common('refresh')} icon={<RefreshCw size={14}/>} onClick={resources.refresh}/></div></header>
    {errors.map(error => <Alert key={error} type="error" title={common(error)} style={{ marginBottom: 12 }}/>) }
    <OverviewFleet data={summary} diagnostics={diagnostics} nodes={resources.nodes.error ? undefined : resources.nodes.data}/>
    <div className="dashboard-main">
      <section className="panel trend-panel usage-workspace"><div className="panel-heading"><h2><Activity/>{text('usage')}</h2><span className="range-caption" title={range}>{data ? range : '—'}</span></div>
        <section className="metrics" aria-label={text('usage')} aria-busy={resources.usage.loading}>
          {(['relay', 'p2p'] as const).map(kind => <article className={`metric${kind === 'relay' ? ' metric-primary' : ''}`} key={kind}><div className="metric-heading">{text(kind)}</div><div className="metric-number" title={raw(traffic(data, kind))}>{kind === 'p2p' && data && !data.p2p_reported ? text('missing') : amount(traffic(data, kind), 'bytes')}</div><div className="duplex"><span>↑ <strong>{bytes(data, `${kind}_upload_bytes`)}</strong></span><span>↓ <strong>{bytes(data, `${kind}_download_bytes`)}</strong></span></div><div className="metric-footer">{comparison(traffic(data, kind), traffic(previous, kind))}</div></article>)}
          <article className="metric"><div className="metric-heading">{text('duration')}</div><div className="metric-number duration" title={raw(exactValue(data, 'connection_ms'), true)}>{amount(exactValue(data, 'connection_ms'), 'duration')}</div><div className="metric-description">{text('durationHint')}</div><div className="metric-footer">{comparison(exactValue(data, 'connection_ms'), exactValue(previous, 'connection_ms'))}</div></article>
          <article className="metric"><div className="metric-heading">{text('connections')}</div><div className="metric-number">{amount(exactValue(data, 'connections'), 'count')}</div><div className="metric-description">{text('connectionsHint')}</div><div className="metric-footer">{comparison(exactValue(data, 'connections'), exactValue(previous, 'connections'))}</div></article>
        </section>
        <OverviewTrend data={trend} daily={period !== 'today'} series={series} lifetime={period === 'lifetime'} onSeries={setSeries}/>
        <div className="transfer-strip" aria-label={text('transfer')}><div className="transfer-title"><Folder/><div><span>{text('transfer')}</span><strong>{amount(totalFiles, 'count')}</strong><small> {text('completed')}</small></div></div><div><span>{text('p2pUpload')} <small>{amount(exactValue(data, 'uploaded_files'), 'count')} {text('files')}</small></span><strong>{bytes(data, 'uploaded_bytes')}</strong></div><div><span>{text('p2pDownload')} <small>{amount(exactValue(data, 'downloaded_files'), 'count')} {text('files')}</small></span><strong>{bytes(data, 'downloaded_bytes')}</strong></div><div className="transfer-results"><span><i className="status-dot amber"/>{text('failed')} <b>{amount(exactValue(data, 'failed_transfers'), 'count')}</b></span><span><i className="status-dot gray"/>{text('cancelled')} <b>{amount(exactValue(data, 'cancelled_transfers'), 'count')}</b></span></div></div>
      </section>
      <aside className="detail-rail"><OverviewRelay data={resources.nodes.data} unavailable={!!resources.nodes.error}/><OverviewPlatform data={diagnostics}/></aside>
    </div>
    <div className="detail-tables"><OverviewRanking data={resources.ranking.error ? undefined : resources.ranking.data} loading={resources.ranking.loading || resources.usage.loading} sort={sort} onSort={setSort} usageLink={usageLink}/><OverviewDevices data={resources.devices.error ? undefined : resources.devices.data} loading={resources.devices.loading} page={page} search={search} attention={attention} onPage={setPage} onSearch={value => { setSearch(value); setPage(1); }} onAttention={value => { setAttention(value); setPage(1); }}/></div>
  </div>;
}
