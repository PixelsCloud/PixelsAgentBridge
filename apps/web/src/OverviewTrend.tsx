import { useContext, useEffect, useId, useRef, useState } from 'react';
import { LanguageContext } from './i18n';
import { byteText, DAY, HOUR, type UsageResult } from './usageModel';
import { chartSamples, compactDuration, type Series } from './overviewModel';
import { useOverviewText } from './overviewText';

export function OverviewTrend({ data, daily, series, lifetime, onSeries }: {
  data?: UsageResult; daily: boolean; series: Series; lifetime: boolean; onSeries: (series: Series) => void;
}) {
  const text = useOverviewText(); const language = useContext(LanguageContext);
  const container = useRef<HTMLDivElement>(null); const svg = useRef<SVGSVGElement>(null);
  const [width, setWidth] = useState(700); const [selected, setSelected] = useState<number>();
  const id = useId().replace(/:/g, '');
  useEffect(() => {
    const observer = new ResizeObserver(entries => { const size = Math.max(280, Math.round(entries[0].contentRect.width)); setWidth(old => old === size ? old : size); });
    if (container.current) observer.observe(container.current);
    return () => observer.disconnect();
  }, []);
  useEffect(() => setSelected(undefined), [data?.from, data?.until, series]);
  const keys = series === 'duration' ? ['connection_ms'] : [`${series}_upload_bytes`, `${series}_download_bytes`];
  const labels = series === 'duration' ? [text('durationTab')] : series === 'p2p' ? [text('p2pUpload'), text('p2pDownload')] : [text('upload'), text('download')];
  const colors = series === 'p2p' ? ['#5099b0', '#a0c9d5'] : ['#238d71', '#95c6b1'];
  const title = text(series === 'duration' ? 'durationTrend' : series === 'p2p' ? 'p2pTrend' : 'relayTrend');
  const samples = chartSamples(data?.points ?? [], keys, daily);
  const maximum = samples.reduce((max, point) => point.values.reduce<bigint>((highest, value) => value !== undefined && value > highest ? value : highest, max), 0n);
  const limit = maximum > 0n ? (maximum * 115n + 99n) / 100n : 1n;
  const height = 180; const left = 82; const right = 16; const top = 20; const bottom = height - 30;
  const originalFrom = data?.from ?? 0; const originalUntil = data?.until ?? 0;
  const fromDate = new Date(originalFrom); const untilDate = new Date(Math.max(originalFrom, originalUntil - 1));
  const start = daily ? new Date(fromDate.getFullYear(), fromDate.getMonth(), fromDate.getDate()).getTime() : originalFrom;
  const end = Math.max(start + (daily ? DAY : HOUR), daily ? new Date(untilDate.getFullYear(), untilDate.getMonth(), untilDate.getDate()).getTime() : originalUntil - HOUR);
  const x = (time: number) => left + (time - start) / (end - start) * (width - left - right);
  const y = (value: bigint) => bottom - Number(value * 100000n / limit) / 100000 * (bottom - top);
  const format = (value: bigint | undefined) => value === undefined ? text('missing') : series === 'duration' ? compactDuration(value, language, 1) : byteText(String(value));
  const date = (value: number, time = false) => new Intl.DateTimeFormat(language, { month: '2-digit', day: '2-digit', ...(time ? { hour: '2-digit', minute: '2-digit', hour12: false } : {}) }).format(value);
  const lines = keys.map((_, seriesIndex) => {
    const runs: typeof samples[] = []; let run: typeof samples = [];
    for (const point of samples) {
      if (point.values[seriesIndex] === undefined || (run.length && point.time - run.at(-1)!.time > (daily ? 26 * HOUR : HOUR) * 1.5)) { if (run.length) runs.push(run); run = []; }
      if (point.values[seriesIndex] !== undefined) run.push(point);
    }
    if (run.length) runs.push(run);
    return runs.map(points => ({ points, path: points.map((point, index) => `${index ? 'L' : 'M'}${x(point.time)},${y(point.values[seriesIndex]!)}`).join(' ') }));
  });
  const active = selected === undefined ? undefined : samples[selected];
  return <>
    <div className="chart-toolbar"><div className="chart-tabs" role="group" aria-label={text('usage')}>
      {(['relay', 'p2p', 'duration'] as const).map(value => <button key={value} aria-pressed={series === value} onClick={() => onSeries(value)}>{text(value === 'duration' ? 'durationTab' : value)}</button>)}
    </div><div className="chart-legend">{labels.map((label, index) => <span key={label}><i style={{ background: colors[index] }}/>{label}</span>)}</div></div>
    <div className="chart-meta"><h3>{title}</h3><span>{data?.from != null && data.until != null ? `${lifetime ? `${text('monthTrend')} · ` : ''}${date(data.from)} — ${date(data.until - 1)}` : '—'}</span><span className="small-badge">{text(daily ? 'daily' : 'hourly')}</span></div>
    <div className="chart-stage" ref={container}>
      <svg ref={svg} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label={title} onPointerLeave={() => setSelected(undefined)} onPointerMove={event => {
        if (!samples.length) return;
        const bounds = event.currentTarget.getBoundingClientRect(); const target = (event.clientX - bounds.left) / bounds.width * width;
        let nearest = 0;
        samples.forEach((point, index) => { if (Math.abs(x(point.time) - target) < Math.abs(x(samples[nearest].time) - target)) nearest = index; });
        setSelected(nearest);
      }}>
        <defs>{colors.map((color, index) => <linearGradient key={color} id={`${id}-${index}`} x1="0" x2="0" y1="0" y2="1"><stop offset="0%" stopColor={color} stopOpacity=".16"/><stop offset="100%" stopColor={color} stopOpacity=".01"/></linearGradient>)}</defs>
        {[0, 1, 2, 3].map(index => { const pos = top + (bottom - top) * (1 - index / 3); return <g key={index}><line x1={left} x2={width - right} y1={pos} y2={pos} className="chart-grid-line"/><text x={left - 12} y={pos + 3} textAnchor="end" className="chart-axis">{format(limit * BigInt(index) / 3n)}</text></g>; })}
        {data?.from != null && data.until != null && [0, 1, 2, 3, 4].map(index => { const time = start + (end - start) * index / 4; return <text key={index} x={x(time)} y={height - 9} textAnchor={index === 0 ? 'start' : index === 4 ? 'end' : 'middle'} className="chart-axis">{daily ? date(time) : new Intl.DateTimeFormat(language, { hour: '2-digit', minute: '2-digit', hour12: false }).format(time)}</text>; })}
        {lines.map((runs, index) => runs.map(({ points, path }, runIndex) => <g key={`${index}-${runIndex}`}>
          {points.length > 1 && <><path d={`${path} L${x(points.at(-1)!.time)},${bottom} L${x(points[0].time)},${bottom}Z`} fill={`url(#${id}-${index})`}/><path d={path} fill="none" stroke={colors[index]} strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke"/></>}
          {points.map(point => <circle key={point.time} cx={x(point.time)} cy={y(point.values[index]!)} r={daily ? 3 : 1.5} fill={colors[index]}/>)}
        </g>))}
        {active && <line className="chart-cursor" x1={x(active.time)} x2={x(active.time)} y1={top} y2={bottom}/>}
        {samples.map((point, index) => <rect key={point.time} className="chart-hit" x={Math.max(left, x(point.time) - 9)} y={top} width="18" height={bottom - top} tabIndex={0} role="button" aria-label={`${date(point.time, !daily)} · ${point.values.map((value, i) => `${labels[i]}: ${format(value)}`).join(', ')}`} onFocus={() => setSelected(index)} onBlur={() => setSelected(undefined)} onKeyDown={event => {
          if (event.key === 'Escape') setSelected(undefined);
          if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); const elements = svg.current?.querySelectorAll<SVGRectElement>('.chart-hit'); elements?.[Math.max(0, Math.min(samples.length - 1, index + (event.key === 'ArrowRight' ? 1 : -1)))]?.focus(); }
        }}/>) }
      </svg>
      {!samples.some(sample => sample.values.some(value => value !== undefined)) && <div className="empty-chart">{text('noReports')}</div>}
      {active && <div className="chart-tooltip" role="tooltip" style={{ top: 20, left: Math.max(7, Math.min(width - 175, x(active.time) + 10)) }}><time>{date(active.time, !daily)}</time>{active.values.map((value, index) => <div className="tooltip-line" key={index}><span><i style={{ background: colors[index] }}/>{labels[index]}</span><strong title={value === undefined ? undefined : String(value)}>{format(value)}</strong></div>)}</div>}
    </div>
    <div className="trend-bottom"><span>{series === 'duration' ? text('durationHint') : `${text(series)} · ${labels.join(' / ')}`}</span><span>{text('units')} · {text(series === 'duration' ? 'durationUnit' : 'byteUnit')}</span></div>
  </>;
}
