import { Alert, Button, Card, Skeleton } from 'antd';
import { Activity, ArrowRight, ArrowUpRight, CircleDot, LayoutGrid, Monitor, Network, ShieldCheck, Users, Wifi, WifiOff } from 'lucide-react';
import { useNavigate } from 'react-router-dom';
import { useText } from './i18n';
import { PageHeading } from './PageHeading';
import { relayCategories, type RelaySnapshot } from './RelaySummary';
import { useLiveRefresh, useResource } from './useResource';
import { AdminOverview } from './AdminOverview';

interface OverviewData {
  total: number;
  online: number;
  offline: number;
  accounts?: number;
  relays?: number;
  relay_status?: RelaySnapshot;
}

export function Overview(props: { liveRevision: number; all?: boolean }) {
  return props.all ? <AdminOverview liveRevision={props.liveRevision}/> : <PersonalOverview {...props}/>;
}

function PersonalOverview({ liveRevision, all = false }: { liveRevision: number; all?: boolean }) {
  const t = useText();
  const navigate = useNavigate();
  const resource = useResource<OverviewData>(`/overview?scope=${all ? 'all' : 'mine'}`);
  useLiveRefresh(resource.refresh, liveRevision);
  const data = resource.error ? undefined : resource.data;
  const hasCounts = data != null && Number.isFinite(data.total) && Number.isFinite(data.online);
  const ratio = hasCounts && data.total > 0 ? Math.min(100, Math.max(0, data.online / data.total * 100)) : 0;
  const stats = [
    { key: 'total', title: 'totalDevices', icon: Monitor, path: '/devices', tone: 'teal', hint: 'statTotalHint' },
    { key: 'online', title: 'online', icon: Wifi, path: '/devices?status=online', tone: 'green', hint: 'statOnlineHint' },
    { key: 'offline', title: 'offline', icon: WifiOff, path: '/devices?status=offline', tone: 'slate', hint: 'statOfflineHint' },
    ...(all ? [
      { key: 'accounts', title: 'totalAccounts', icon: Users, path: '/accounts', tone: 'violet', hint: 'statAccountsHint' },
    ] : []),
  ];
  const shortcuts = [
    { title: all ? 'devices' : 'myDevices', description: 'shortcutDevices', icon: Monitor, path: '/devices' },
    { title: all ? 'serviceUsage' : 'myUsage', description: 'shortcutUsage', icon: Activity, path: '/usage' },
    ...(all ? [
      { title: 'accounts', description: 'shortcutAccounts', icon: Users, path: '/accounts' },
      { title: 'events', description: 'shortcutAudit', icon: ShieldCheck, path: '/audit' },
    ] : [
      { title: 'savedDevices', description: 'savedDevicesHint', icon: Network, path: '/saved-devices' },
      { title: 'account', description: 'accountPageHint', icon: ShieldCheck, path: '/account' },
    ]),
  ];

  return <div className="overview-page page-stack">
    <PageHeading title={t('overview')} description={t(all ? 'overviewDescription' : 'personalOverviewDescription')}/>
    {resource.error && <Alert type="error" title={t(resource.error)} action={<Button onClick={resource.refresh}>{t('retry')}</Button>}/>}
    <div className={`overview-stats${all ? ' overview-stats-admin' : ''}`}>
      {stats.map(({ key, title, icon: Icon, path, tone, hint }) => <Card className={`metric-card metric-${tone}`} key={key}>
        <div className="metric-top"><span>{t(title)}</span><span className="metric-icon"><Icon size={18}/></span></div>
        {resource.loading && !resource.data ? <Skeleton.Input active size="small"/> : <Button className="metric-value" type="link" aria-label={t(title)} onClick={() => navigate(path)}>{data?.[key as keyof Omit<OverviewData, 'relay_status'>] ?? '—'}<ArrowUpRight size={18}/></Button>}
        <span className="metric-hint">{t(hint)}</span>
      </Card>)}
    </div>
    <div className="overview-main-grid">
      <Card className="connectivity-card" title={<span className="section-title"><CircleDot size={18}/>{t('deviceConnectivity')}</span>} extra={<span className="section-kicker">{t('currentSnapshot')}</span>}>
        <div className="connectivity-body">
          <div className="connectivity-chart" role="img" aria-label={`${t('online')}: ${hasCounts ? data.online : '—'} / ${hasCounts ? data.total : '—'}`}>
            <svg viewBox="0 0 200 200" aria-hidden="true"><circle className="ring-track" cx="100" cy="100" r="82"/><circle className="ring-value" cx="100" cy="100" r="82" pathLength="100" strokeDasharray={`${ratio} 100`} transform="rotate(-90 100 100)"/></svg>
            <div><strong>{hasCounts && data.total > 0 ? `${Math.round(ratio)}%` : '—'}</strong><span>{t('onlineShare')}</span></div>
          </div>
          <div className="connectivity-details">
            <p className="section-description">{t('connectivityDescription')}</p>
            <button className="connection-legend" onClick={() => navigate('/devices?status=online')}><span><i className="legend-dot online"/>{t('online')}</span><strong>{hasCounts ? data.online : '—'}</strong><ArrowRight size={15}/></button>
            <button className="connection-legend" onClick={() => navigate('/devices?status=offline')}><span><i className="legend-dot offline"/>{t('offline')}</span><strong>{hasCounts ? data.offline : '—'}</strong><ArrowRight size={15}/></button>
          </div>
        </div>
        <div className="card-note"><CircleDot size={14}/><span>{t('deviceOnlineHint')}</span></div>
      </Card>
      <Card className="quick-access-card" title={<span className="section-title"><LayoutGrid size={18}/>{t('quickAccess')}</span>}>
        <div className="quick-access-list">{shortcuts.map(({ title, description, icon: Icon, path }) => <button className="quick-access-item" key={path} onClick={() => navigate(path)}>
          <span className="shortcut-icon"><Icon size={19}/></span><span><strong>{t(title)}</strong><small>{t(description)}</small></span><ArrowUpRight size={17}/>
        </button>)}</div>
      </Card>
    </div>
    {all && <section className="overview-relay-strip" aria-label={t('nodes')}>
      <div className="overview-relay-title"><Network size={18}/><strong>{t('nodes')}</strong></div>
      <dl className="overview-relay-metrics">{relayCategories.map(category => <div className={`relay-metric relay-metric-${category}${category === 'abnormal' && (data?.relay_status?.summary.abnormal ?? 0) > 0 ? ' has-warning' : ''}`} key={category}>
        <dt>{t(`relayCount.${category}`)}</dt>
        <dd>{data?.relay_status?.summary[category] ?? (category === 'configured' ? data?.relays : undefined) ?? '—'}</dd>
      </div>)}</dl>
    </section>}
  </div>;
}
