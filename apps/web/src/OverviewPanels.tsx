import { useContext, useState } from 'react';
import { Activity, Command, Cpu, Layers, Monitor, Network, Server, Terminal, Users } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { LanguageContext, useDate, useText } from './i18n';
import { ConsoleSelect } from './ConsoleSelect';
import { byteText } from './usageModel';
import { compactDuration, type Diagnostics, type Nodes, type OverviewData } from './overviewModel';
import { useOverviewText } from './overviewText';

export function OverviewFleet({ data, diagnostics, nodes }: { data?: OverviewData; diagnostics?: Diagnostics; nodes?: Nodes }) {
  const text = useOverviewText(); const navigate = useNavigate();
  return <section className="fleet-overview overview-stats" aria-label={text('registered')}>
    <article className="fleet-card"><span className="fleet-icon"><Users/></span><div className="fleet-main"><div className="fleet-value"><span>{text('registered')}</span><button className="overview-count" onClick={() => navigate('/accounts')}>{data?.accounts ?? '—'}</button><small>{text('administrators')} {data?.account_summary?.administrators ?? '—'}</small></div><div className="fleet-details"><span>{text('enabled')} <b>{data?.account_summary?.enabled ?? '—'}</b></span><span>{text('disabled')} <b>{data?.account_summary?.disabled ?? '—'}</b></span></div></div></article>
    <article className="fleet-card"><span className="fleet-icon"><Monitor/></span><div className="fleet-main"><div className="fleet-value"><span>{text('devices')}</span><button className="metric-value" aria-label={text('devices')} onClick={() => navigate('/devices')}>{data?.total ?? '—'}</button></div><div className="fleet-details"><span>{text('reported')} <b>{diagnostics?.reported ?? '—'}</b></span><span>{text('unreported')} <b>{diagnostics?.not_reported ?? '—'}</b></span></div></div></article>
    <article className="fleet-card"><span className="fleet-icon"><Activity/></span><div className="fleet-main"><div className="fleet-value connection-values"><span><i className="status-dot"/>{text('online')} <button className="overview-count" aria-label={text('online')} onClick={() => navigate('/devices?status=online')}>{data?.online ?? '—'}</button></span><span><i className="status-dot gray"/>{text('offline')} <button className="overview-count" aria-label={text('offline')} onClick={() => navigate('/devices?status=offline')}>{data?.offline ?? '—'}</button></span></div><div className="fleet-details">{text('control')}</div></div></article>
    <article className="fleet-card"><span className="fleet-icon"><Network/></span><div className="fleet-main"><div className="fleet-value"><span>{text('relayNodes')}</span><button className="overview-count" aria-label={text('relayNodes')} onClick={() => navigate('/relay')}>{nodes?.summary.configured ?? '—'}</button></div><div className="fleet-details"><span>{text('relayAvailable')} <b>{nodes?.summary.available ?? '—'}</b></span><span>{text('maintenance')} <b>{nodes?.summary.maintenance ?? '—'}</b></span><span>{text('abnormal')} <b>{nodes?.summary.abnormal ?? '—'}</b></span></div></div></article>
  </section>;
}

export function OverviewPlatform({ data }: { data?: Diagnostics }) {
  const text = useOverviewText(); const [view, setView] = useState<'systems' | 'architectures' | 'versions'>('systems');
  const platforms = [
    { key: 'windows', label: 'Windows', Icon: Monitor, color: '#438fba' },
    { key: 'linux', label: 'Linux', Icon: Terminal, color: '#6a8e7d' },
    { key: 'macos', label: 'macOS', Icon: Command, color: '#8a93a0' },
    { key: 'android', label: 'Android', Icon: Cpu, color: '#71a970' },
  ];
  const rows = [...platforms.map(platform => ({ ...platform, count: data?.systems.find(group => group.value === platform.key)?.count ?? (data ? 0 : undefined) })), ...(data?.systems.filter(group => !platforms.some(platform => platform.key === group.value)) ?? []).map(group => ({ key: group.value ?? 'unknown', label: group.value ?? text('unknown'), Icon: Monitor, color: '#a7b4ad', count: group.count }))];
  return <section className="panel platform-panel" aria-label={text('platforms')}><div className="panel-heading"><h2><Layers/>{text('platforms')}</h2><div className="mini-tabs" role="group" aria-label={text('platforms')}>{(['systems', 'architectures', 'versions'] as const).map(value => <button key={value} aria-pressed={view === value} onClick={() => setView(value)}>{text(value)}</button>)}</div></div>
    <div className="platform-body">
      {view === 'systems' && <div className="platform-list">{rows.map(({ key, label, Icon, color, count }) => { const share = data?.total ? Math.round((count ?? 0) / data.total * 100) : 0; return <div className="platform-row" key={key} data-platform={key}><span className="platform-symbol" style={{ color }}><Icon/></span><span className="platform-name">{label}</span><span className="platform-track" aria-hidden="true"><i style={{ width: `${share}%`, background: color }}/></span><strong>{count ?? '—'}</strong><small>{data ? `${share}%` : '—'}</small></div>; })}</div>}
      {view === 'architectures' && <div className="architecture-list">{(data?.architectures ?? []).map(group => <div className="architecture-item" key={group.value ?? 'unknown'}><div><span><Cpu/>{group.value ?? text('unknown')}</span><strong>{group.count}</strong></div><div className="architecture-track"><i style={{ width: `${data?.total ? group.count / data.total * 100 : 0}%` }}/></div></div>)}</div>}
      {view === 'versions' && <div className="version-list">{(data?.versions ?? []).map(group => <div className="version-row" key={group.value ?? 'unknown'}><span>{group.value ?? text('missing')}</span><span className="platform-track"><i style={{ width: `${data?.total ? group.count / data.total * 100 : 0}%`, background: '#7d9c8a' }}/></span><strong>{group.count}</strong></div>)}{data?.versions_truncated && <span className="section-count">100+</span>}</div>}
    </div>
    {view === 'systems' && <div className="platform-insights"><div><span>{text('architectures')}</span><strong>{data?.architectures.map(group => `${group.value ?? text('unknown')} ${group.count}`).join(' / ') ?? '—'}</strong></div><div><span>{text('client')}</span><strong>{data?.versions.slice(0, 3).map(group => `${group.value ?? text('missing')} · ${group.count}`).join(' / ') ?? '—'}</strong></div></div>}
  </section>;
}

export function OverviewRelay({ data, unavailable }: { data?: Nodes; unavailable: boolean }) {
  const text = useOverviewText(); const common = useText(); const language = useContext(LanguageContext); const date = useDate();
  const [selected, setSelected] = useState<string>();
  const node = data?.items.find(item => item.id === selected) ?? data?.items[0];
  const valid = !unavailable && node?.online && node.sample_current_instance !== false && node.telemetry_current === true;
  const number = (value?: string | null) => valid && value != null && /^\d+$/.test(value) ? BigInt(value) : undefined;
  const bytes = (value?: string | null) => number(value) === undefined ? '—' : byteText(String(number(value)));
  const server = !unavailable && data?.health?.server === 'responding'; const database = !unavailable && data?.health?.database === 'reachable';
  const policy = !unavailable && node?.online && node.applied_policy_version != null && node.offered_policy_version != null;
  return <section className="panel node-panel" aria-label={text('nodes')}><div className="panel-heading"><h2><Network/>{text('nodes')}</h2><span className="small-badge">{!unavailable && data ? `${data.summary.available} / ${data.summary.configured}` : '—'}</span></div>
    <div className="node-title"><span><i className={`status-dot${!unavailable && node?.online ? '' : ' gray'}`}/>{data && data.items.length > 1 ? <ConsoleSelect aria-label={common('nodes')} value={node?.id} onChange={setSelected} options={data.items.map(item => ({ value: item.id, label: item.id }))}/> : <strong>{node?.id ?? text('noNodes')}</strong>}</span><small>{node?.agent_version ? `v${node.agent_version}` : text('missing')}</small></div>
    <div className="node-live"><div><span>{text('currentConnections')}</span><strong>{number(node?.connections_exact)?.toLocaleString(language) ?? '—'}</strong></div><div><span>{text('capacity')}</span><strong className="node-text">{node?.capacity_mode === 'host' || node?.capacity_mbps === 0 ? common('relayHostBandwidth') : node?.capacity_mbps ? `${node.capacity_mbps} Mbps` : text('notConfigured')}</strong></div></div>
    <div className="node-traffic"><div><span>{text('received')}</span><strong>{bytes(node?.uploaded_bytes_exact)}</strong></div><div><span>{text('sent')}</span><strong>{bytes(node?.downloaded_bytes_exact)}</strong></div></div>
    <div className="node-footer"><span>{policy ? text(node!.applied_policy_version === node!.offered_policy_version ? 'policyReady' : 'policyPending') : text('missing')}</span><span title={node?.last_seen ? date(node.last_seen) : undefined}>{!unavailable && node?.online && node.heartbeat_age_ms != null ? `${compactDuration(BigInt(Math.max(0, node.heartbeat_age_ms)), language, 1)} ${text('heartbeat')}` : '—'}</span></div>
    <div className="service-status"><span>{text('service')} <i className={`status-dot${server ? '' : ' amber'}`}/><b>{text(server ? 'healthy' : 'check')}</b></span><span>{text('database')} <i className={`status-dot${database ? '' : ' amber'}`}/><b>{text(database ? 'healthy' : 'check')}</b></span></div>
    <div className="service-counters"><span>{text('available')}</span><span>{text('maintenance')} {unavailable ? '—' : data?.summary.maintenance ?? '—'} · {text('abnormal')} {unavailable ? '—' : data?.summary.abnormal ?? '—'}</span></div>
  </section>;
}
