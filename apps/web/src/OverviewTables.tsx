import { useContext } from 'react';
import { ChevronLeft, ChevronRight, Monitor, Search, Users } from 'lucide-react';
import { Link } from 'react-router-dom';
import { LanguageContext, useDate, useText } from './i18n';
import { ConsoleSelect } from './ConsoleSelect';
import { byteText, durationText } from './usageModel';
import { compactDuration, exactValue, type Devices, type Ranking, type RankingMetric, type RankingRow } from './overviewModel';
import { useOverviewText } from './overviewText';

export function OverviewRanking({ data, loading, sort, onSort, usageLink }: {
  data?: Ranking; loading: boolean; sort: RankingMetric; onSort: (value: RankingMetric) => void; usageLink: string;
}) {
  const text = useOverviewText(); const common = useText(); const language = useContext(LanguageContext); const date = useDate();
  const rows = data?.items ?? [];
  const maximum = rows.reduce((max, row) => { const value = exactValue(row, sort) ?? 0n; return value > max ? value : max; }, 0n);
  const amount = (row: RankingRow, key: RankingMetric) => {
    const value = key.startsWith('p2p_') && !row.p2p_reported ? undefined : exactValue(row, key);
    return <td><span className="rank-amount" title={value === undefined ? undefined : key === 'connection_ms' ? durationText(String(value), language) : `${value} B`}>{value === undefined ? '—' : key === 'connection_ms' ? compactDuration(value, language) : byteText(String(value))}</span>{key === sort && maximum > 0n && value !== undefined && <i className="rank-meter" style={{ width: `${Number(value * 10000n / maximum) / 100}%` }}/>}</td>;
  };
  return <section className="panel ranking-panel" aria-label={text('ranking')} aria-busy={loading}>
    <div className="panel-heading"><h2><Users/>{text('ranking')}<span className="section-count">TOP 5</span></h2><ConsoleSelect aria-label={text('rankingSort')} value={sort} onChange={onSort} options={[{ value: 'relay_download_bytes', label: text('byRelay') }, { value: 'p2p_download_bytes', label: text('byP2p') }, { value: 'connection_ms', label: text('byDuration') }]}/></div>
    <div className="table-scroll"><table className="data-table ranking-table"><thead><tr><th>{text('user')}</th><th>{text('relayDown')}</th><th>{text('p2pDown')}</th><th>{text('durationTab')}</th></tr></thead><tbody>{rows.map((row, index) => {
      const name = row.username ?? (row.subject === 'guest' ? text('guest') : text('unknown'));
      return <tr key={row.subject}><td><div className="rank-user"><span className="rank-index">{String(index + 1).padStart(2, '0')}</span><span className="rank-avatar">{name.slice(0, 1).toLocaleUpperCase()}</span>{row.user_id ? <Link className="rank-name" title={name} to={`/accounts/${encodeURIComponent(row.user_id)}`}>{name}</Link> : <span className="rank-name" title={name}>{name}</span>}</div></td>{amount(row, 'relay_download_bytes')}{amount(row, 'p2p_download_bytes')}{amount(row, 'connection_ms')}</tr>;
    })}{!rows.length && <tr><td colSpan={4} className="empty-row">{loading ? common('loading') : text('noReports')}</td></tr>}</tbody></table></div>
    <div className="table-footer"><span>{data?.from != null && data.until != null ? `${date(data.from)} — ${date(data.until)}` : text('sinceStart')}</span><Link to={usageLink}>{text('allUsage')}<ChevronRight/></Link></div>
  </section>;
}

export function OverviewDevices({ data, loading, page, search, attention, onPage, onSearch, onAttention }: {
  data?: Devices; loading: boolean; page: number; search: string; attention: boolean;
  onPage: (page: number) => void; onSearch: (search: string) => void; onAttention: (value: boolean) => void;
}) {
  const text = useOverviewText(); const common = useText(); const date = useDate();
  const total = data?.total; const actualPage = data?.page ?? page; const size = data?.page_size ?? 4;
  const pages = Math.max(1, Math.ceil((total ?? 0) / size));
  const names: Record<string, string> = { windows: 'Windows', linux: 'Linux', macos: 'macOS', android: 'Android' };
  return <section className="panel device-panel" aria-label={text('deviceDetails')} aria-busy={loading}>
    <div className="panel-heading"><h2><Monitor/>{text('deviceDetails')}</h2><label className="device-search"><Search/><input type="search" maxLength={128} value={search} onChange={event => onSearch(event.target.value)} placeholder={text('deviceSearch')} aria-label={text('deviceSearch')}/></label></div>
    <div className="device-toolbar"><div className="mini-tabs" role="group" aria-label={text('deviceDetails')}>{[false, true].map(value => <button key={String(value)} aria-pressed={attention === value} onClick={() => onAttention(value)}>{text(value ? 'attention' : 'allDevices')}</button>)}</div><span>{text('reportOrder')}</span></div>
    <div className="table-scroll"><table className="data-table device-table"><thead><tr><th>{text('deviceOwner')}</th><th>{text('system')}</th><th>{text('client')}</th><th>{text('reportAt')}</th></tr></thead><tbody>{data?.items.map(device => <tr key={device.id}>
      <td><div className="device-name"><i className={`status-dot${device.online ? '' : ' gray'}`} title={text(device.online ? 'connected' : 'disconnected')}/><Link to={`/devices/${encodeURIComponent(device.id)}`} title={device.name}><strong>{device.name}</strong></Link></div><small className="device-secondary">{device.associated_username ?? text('unbound')}</small></td>
      <td><span className="device-platform">{names[device.system ?? ''] ?? device.os_name ?? text('unknown')}</span><small className="device-secondary">{device.architecture ?? text('unknown')}</small></td><td><span className="device-client">{device.agent_version ?? text('missing')}</span></td><td><span className="device-report">{device.diagnostics?.reported_at != null ? date(device.diagnostics.reported_at) : text('missing')}</span></td>
    </tr>)}{!data?.items.length && <tr><td colSpan={4} className="empty-row">{loading ? common('loading') : text('noDevices')}</td></tr>}</tbody></table></div>
    <div className="table-footer"><span>{total === undefined ? '—' : total ? `${(actualPage - 1) * size + 1}–${Math.min(actualPage * size, total)} / ${total}` : '0'}</span><div className="table-paging"><button aria-label={text('previousPage')} disabled={loading || actualPage <= 1} onClick={() => onPage(actualPage - 1)}><ChevronLeft/></button><span>{actualPage} / {pages}</span><button aria-label={text('nextPage')} disabled={loading || actualPage >= pages} onClick={() => onPage(actualPage + 1)}><ChevronRight/></button><Link to="/devices">{text('allDevices')}<ChevronRight/></Link></div></div>
  </section>;
}
