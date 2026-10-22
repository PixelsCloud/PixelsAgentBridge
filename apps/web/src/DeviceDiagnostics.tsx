import { Alert, Button, Card, Skeleton } from 'antd';
import { Cpu, Monitor, RefreshCw } from 'lucide-react';
import { useText, useDate } from './i18n';
import { useLiveRefresh, useResource } from './useResource';
interface Group { value: string | null; count: number }
interface Diagnostics {
  total: number; online?: number; offline?: number; reported: number; not_reported: number; sampled_at: number;
  systems: Group[]; architectures: Group[];
}
export function DeviceDiagnostics({ admin, liveRevision }: { admin: boolean; liveRevision: number }) {
  const t = useText(); const date = useDate();
  const resource = useResource<Diagnostics>(`/devices/diagnostics?scope=${admin ? 'all' : 'mine'}`);
  useLiveRefresh(resource.refresh, liveRevision);
  const value = resource.error ? undefined : resource.data;
  const ratio = value && value.total > 0 ? Math.min(100, Math.max(0, value.reported / value.total * 100)) : 0;
  const pending = resource.loading && !value;
  const groups = [
    { kind: 'systems', label: t('system'), Icon: Monitor },
    { kind: 'architectures', label: t('architecture'), Icon: Cpu },
  ] as const;
  const systems: Record<string, string> = { windows: 'Windows', linux: 'Linux', macos: 'macOS', android: 'Android' };
  return <Card className="diagnostics-card"><section aria-label={t('deviceDiagnostics')}>
    <header className="diagnostics-heading"><div><strong>{t('deviceDiagnostics')}</strong><span>{t(admin ? 'deviceDiagnosticsScope' : 'myDevices')}</span></div>
      <Button type="text" size="small" aria-label={t('deviceRefreshDiagnostics')} icon={<RefreshCw size={14}/>} onClick={resource.refresh} loading={resource.loading}/>
    </header>
    {resource.error && <Alert type="error" title={t(resource.error)} showIcon/>}
    <div className="diagnostics-metrics">
      {(['total', 'online', 'offline'] as const).map(key => <div className={`diagnostic-metric diagnostic-metric-${key}`} key={key}>
        <span title={key === 'online' ? t('deviceOnlineHint') : undefined}>{t(key === 'total' ? 'totalDevices' : key)}</span>
        {pending ? <Skeleton.Input active size="small"/> : <strong>{value?.[key] ?? '—'}</strong>}
      </div>)}
      <div className="diagnostic-metric diagnostic-reporting">
        <span>{t('diagnostics.reported')}</span>
        {pending ? <Skeleton.Input active size="small"/> : <strong>{value?.reported ?? '—'}<small> / {value?.total ?? '—'}</small></strong>}
        <div className="diagnostic-report-track" role="img" aria-label={`${t('diagnostics.reported')}: ${value?.reported ?? '—'} / ${value?.total ?? '—'}`}><i style={{ width: `${ratio}%` }}/></div>
        <span className="diagnostic-report-hint">{t('diagnostics.not_reported')} {value?.not_reported ?? '—'}</span>
      </div>
    </div>
    <div className="diagnostics-distributions">{groups.map(({ kind, label, Icon }) => <div className="diagnostics-distribution" role="group" aria-label={label} key={kind}>
      <span className="diagnostics-distribution-title"><Icon size={15}/>{label}</span>
      <div className="diagnostics-chips">
        {(value?.[kind] ?? []).map(group => {
          const name = group.value === null ? t('unknown') : kind === 'systems' ? systems[group.value] ?? group.value : group.value;
          return <span className="diagnostic-chip" key={group.value ?? 'unknown'}>
            <span>{name}</span><b>{group.count}</b>
          </span>;
        })}
        {!pending && !resource.error && !value?.[kind]?.length && <span className="diagnostics-empty">{t('noData')}</span>}
        {pending && <Skeleton.Input active size="small"/>}
      </div>
    </div>)}</div>
    {value?.sampled_at ? <footer className="diagnostics-footer"><time>{date(value.sampled_at)}</time></footer> : null}
  </section></Card>;
}
