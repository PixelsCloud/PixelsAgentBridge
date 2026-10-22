import type { UsageResult, Counters, UsagePoint } from './usageModel';
import type { RelaySnapshot } from './RelaySummary';
import type { Device } from './DevicePages';

export type Period = 'today' | '7' | 'month' | 'lifetime';
export type Series = 'relay' | 'p2p' | 'duration';
export type RankingMetric = 'relay_download_bytes' | 'p2p_download_bytes' | 'connection_ms';
export interface OverviewData {
  total: number; online: number; offline: number; accounts?: number;
  account_summary?: { enabled: number; disabled: number; administrators: number };
  relay_status?: RelaySnapshot;
}
export interface Group { value: string | null; count: number }
export interface Diagnostics {
  total: number; reported: number; not_reported: number; sampled_at: number;
  systems: Group[]; architectures: Group[]; versions: Group[]; versions_truncated?: boolean;
}
export interface OverviewNode {
  id: string; online: boolean; configured?: boolean; agent_version?: string | null;
  capacity_mbps?: number | null; capacity_mode?: string | null;
  telemetry_current?: boolean; sample_current_instance?: boolean;
  connections_exact?: string | null; uploaded_bytes_exact?: string | null; downloaded_bytes_exact?: string | null;
  applied_policy_version?: number | null; offered_policy_version?: number | null;
  heartbeat_age_ms?: number | null; last_seen?: number | null;
}
export interface Nodes extends RelaySnapshot { items: OverviewNode[]; total: number }
export interface Devices { items: Device[]; total: number; page: number; page_size: number }
export interface RankingRow { subject: string; user_id: string | null; username: string | null; counters_exact: Counters; p2p_reported?: boolean; updated_at: number | null }
export interface Ranking { items: RankingRow[]; from: number | null; until: number | null; interval: string }
export type CountData = { counters?: Counters; counters_exact?: Counters; updated_at?: number | null };

export function exactValue(data: CountData | undefined, key: string): bigint | undefined {
  if (!data || data.updated_at === null) return undefined;
  const value = data.counters_exact?.[key] ?? data.counters?.[key];
  if (typeof value === 'string' && /^\d+$/.test(value)) return BigInt(value);
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : undefined;
}
export function addValues(left: bigint | undefined, right: bigint | undefined) {
  return left === undefined || right === undefined ? undefined : left + right;
}
export function traffic(data: UsageResult | undefined, kind: 'relay' | 'p2p') {
  if (kind === 'p2p' && !data?.p2p_reported) return undefined;
  return addValues(exactValue(data, `${kind}_upload_bytes`), exactValue(data, `${kind}_download_bytes`));
}
export function compactDuration(value: bigint, language: string, maximumParts = 2) {
  const unit = (count: bigint, name: string) => new Intl.NumberFormat(language, { style: 'unit', unit: name, unitDisplay: 'short', maximumFractionDigits: 0 }).format(count);
  if (value === 0n) return unit(0n, 'second');
  if (value < 1000n) return `< ${unit(1n, 'second')}`;
  let remaining = value / 1000n;
  const parts: string[] = [];
  for (const [name, scale] of [['day', 86400n], ['hour', 3600n], ['minute', 60n], ['second', 1n]] as const) {
    const count = remaining / scale; remaining %= scale;
    if (count) parts.push(unit(count, name));
  }
  return parts.slice(0, maximumParts).join(' ');
}
export function previousMatches(current: UsageResult | undefined, previous: UsageResult | undefined) {
  return current?.from != null && current.until != null && previous?.until === current.from
    && previous.from === current.from - (current.until - current.from) ? previous : undefined;
}
export function percentChange(value: bigint | undefined, before: bigint | undefined) {
  if (value === undefined || before === undefined || before === 0n) return undefined;
  const difference = (value - before) * 1000n / before;
  const magnitude = difference < 0n ? -difference : difference;
  return `${difference > 0n ? '+' : difference < 0n ? '−' : ''}${magnitude / 10n}${magnitude % 10n ? `.${magnitude % 10n}` : ''}%`;
}
export interface Sample { time: number; values: (bigint | undefined)[] }
export function chartSamples(points: UsagePoint[], keys: string[], daily: boolean): Sample[] {
  const groups = new Map<number, (bigint | undefined)[]>();
  for (const point of points) {
    const local = new Date(point.hour_unix_ms);
    const time = daily ? new Date(local.getFullYear(), local.getMonth(), local.getDate()).getTime() : point.hour_unix_ms;
    if (!groups.has(time)) groups.set(time, keys.map(() => undefined));
    const values = groups.get(time)!;
    keys.forEach((key, index) => {
      if (key.startsWith('p2p_') && !point.p2p_reported) return;
      const value = exactValue(point, key);
      if (value !== undefined) values[index] = (values[index] ?? 0n) + value;
    });
  }
  return [...groups].sort(([a], [b]) => a - b).map(([time, values]) => ({ time, values }));
}
