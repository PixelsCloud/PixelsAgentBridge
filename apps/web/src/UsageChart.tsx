import { useContext } from 'react';
import { Card, Empty } from 'antd';
import { Activity } from 'lucide-react';
import { LanguageContext, useDate, useText } from './i18n';
import { byteText, counter, type UsagePoint } from './usageModel';

/** Relative bar heights use integer arithmetic; tooltips keep exact byte counts. */
export function UsageChart({ points }: { points: UsagePoint[] }) {
  const t = useText(); const date = useDate(); const language = useContext(LanguageContext);
  const series = ['relay_upload_bytes', 'relay_download_bytes', 'p2p_upload_bytes', 'p2p_download_bytes'] as const;
  const samples = points.slice(-72).map(point => ({
    time: point.hour_unix_ms,
    values: series.map(key => {
      if (key.startsWith('p2p_') && !point.p2p_reported) return undefined;
      if (point.counters_exact?.[key] === undefined && point.counters?.[key] === undefined) return undefined;
      const value = counter(point.counters_exact, point.counters, key);
      return value === undefined ? undefined : BigInt(value);
    }),
  }));
  const maximum = samples.reduce((max, sample) => sample.values.reduce<bigint>((value, next) => next !== undefined && next > value ? next : value, max), 0n);
  const hasData = samples.some(sample => sample.values.some(value => value !== undefined));
  const tick = (value: number) => new Date(value).toLocaleString(language, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });

  return <Card className="usage-chart-card" title={<span className="section-title"><Activity size={18}/>{t('usageChartTitle')}</span>}>
    <div className="usage-chart-heading"><span>{t('usageChartHint')}</span><div className="usage-chart-legend">{series.map((key, index) => <span key={key}><i className={`usage-series-${index}`}/>{t(key)}</span>)}</div></div>
    {hasData ? <div className="usage-chart" role="img" aria-label={`${t('usageChartTitle')}: ${date(samples[0].time)} — ${date(samples.at(-1)!.time)}`}>
      <div className="usage-chart-axis"><span>{byteText(String(maximum))}</span><span>{byteText(String(maximum / 2n))}</span><span>0 B</span></div>
      <div className="usage-chart-plot"><div className="usage-chart-grid"><i/><i/><i/></div><div className={`usage-chart-bars${samples.length > 24 ? ' usage-chart-dense' : ''}`} style={{ gridTemplateColumns: `repeat(${samples.length}, minmax(0, 1fr))` }}>{samples.map((sample, position) => <div className="usage-chart-bucket" key={`${sample.time}-${position}`}>
        {sample.values.map((value, index) => <div key={index} className={`usage-chart-bar usage-series-${index}`} style={{ height: value === undefined || maximum === 0n ? '0' : `${Number(value * 10000n / maximum) / 100}%` }}
          title={`${date(sample.time)} · ${t(series[index])}: ${value === undefined ? t('unknown') : `${byteText(String(value))} (${value} B)`}`}/>)}
      </div>)}</div></div>
      <div className="usage-chart-dates"><span>{tick(samples[0].time)}</span><span>{tick(samples.at(-1)!.time)}</span></div>
    </div> : <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('usagePending')}/>}
  </Card>;
}
