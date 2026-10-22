export const HOUR = 3_600_000;
export const DAY = 86_400_000;
export const metrics = ['relay_upload_bytes', 'relay_download_bytes', 'p2p_upload_bytes', 'p2p_download_bytes', 'connections', 'connection_ms', 'uploaded_files', 'downloaded_files', 'uploaded_bytes', 'downloaded_bytes', 'failed_transfers', 'cancelled_transfers'] as const;
export type Counters = Record<string, string | number | boolean>;
export interface UsagePoint { hour_unix_ms: number; counters?: Counters; counters_exact?: Counters; incomplete?: boolean; p2p_reported?: boolean }
export interface UsageResult {
  from: number | null; until: number | null; interval: string; timezone: string;
  counters: Counters; counters_exact?: Counters; incomplete?: boolean; p2p_reported?: boolean; points: UsagePoint[]; updated_at: number | null;
}
export interface UsageWindow { from: number; until: number; interval: 'hour' | 'day' | 'lifetime'; error?: string }

export function usageWindow(params: URLSearchParams, now = new Date()): UsageWindow {
  const period = params.get('period') ?? 'today';
  const requested = params.get('interval') ?? 'auto';
  if (!['auto', 'hour', 'day', 'lifetime'].includes(requested)) return { from: 0, until: 0, interval: 'hour', error: 'invalid_input' };
  if (period === 'lifetime' || requested === 'lifetime') return { from: 0, until: 0, interval: 'lifetime' };
  const days = Number(period);
  const dailyPreset = requested === 'day' || (requested === 'auto' && days > 90);
  const bucket = dailyPreset ? DAY : HOUR;
  const until = Number(params.get('until') ?? (Math.floor(now.getTime() / bucket) * bucket + bucket));
  const localStart = new Date(now.getFullYear(), now.getMonth(), period === 'month' ? 1 : now.getDate()).getTime();
  const from = Number(params.get('from') ?? (period === 'today' || period === 'month' ? localStart : until - days * DAY));
  if (!Number.isSafeInteger(from) || !Number.isSafeInteger(until) || from < 0 || until <= from || until > now.getTime() + DAY) {
    return { from, until, interval: 'hour', error: 'invalid_input' };
  }
  const hourlyOldest = Math.ceil((now.getTime() - 90 * DAY) / HOUR) * HOUR;
  const interval = requested === 'day' || (requested === 'auto' && (from < hourlyOldest || until - from > 90 * DAY)) ? 'day' : 'hour';
  const size = interval === 'day' ? DAY : HOUR;
  const aligned = { from: Math.floor(from / size) * size, until: Math.ceil(until / size) * size, interval } as UsageWindow;
  if (aligned.from < Math.ceil((now.getTime() - (interval === 'day' ? 730 : 90) * DAY) / size) * size) aligned.error = 'usage_range_unavailable';
  return aligned;
}

export function usageQuery(params: URLSearchParams, window: UsageWindow, all: boolean) {
  const query = new URLSearchParams({ scope: all ? 'all' : 'mine', interval: window.interval });
  if (window.interval !== 'lifetime') { query.set('from', String(window.from)); query.set('until', String(window.until)); }
  if (all) {
    query.set('population', params.get('population') ?? 'all');
    if (params.get('user')) query.set('user', params.get('user')!);
  }
  return query;
}

export function counter(exact: Counters | undefined, legacy: Counters | undefined, key: string): string | undefined {
  const value = exact?.[key];
  if (typeof value === 'string' && /^\d+$/.test(value)) return value;
  const fallback = legacy?.[key] ?? 0;
  return typeof fallback === 'number' && Number.isSafeInteger(fallback) && fallback >= 0 ? String(fallback) : undefined;
}

export function byteText(value: string): string {
  const bytes = BigInt(value);
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB'];
  let scale = 1n; let index = 0;
  while (bytes >= scale * 1024n && index < units.length - 1) { scale *= 1024n; index++; }
  if (index === 0) return `${value} B`;
  let rounded = (bytes * 100n + scale / 2n) / scale;
  if (rounded >= 102400n && index < units.length - 1) { scale *= 1024n; index++; rounded = (bytes * 100n + scale / 2n) / scale; }
  const fraction = String(rounded % 100n).padStart(2, '0').replace(/0+$/, '');
  return `${rounded / 100n}${fraction ? `.${fraction}` : ''} ${units[index]}`;
}

export function durationText(value: string, language = 'en'): string {
  const milliseconds = BigInt(value);
  const part = (count: bigint, unit: string) => new Intl.NumberFormat(language, { style: 'unit', unit, unitDisplay: 'short', maximumFractionDigits: 0 }).format(count);
  if (milliseconds === 0n) return part(0n, 'second');
  if (milliseconds < 1000n) return `< ${part(1n, 'second')}`;
  let remaining = milliseconds / 1000n;
  const parts: string[] = [];
  for (const [unit, scale] of [['day', 86400n], ['hour', 3600n], ['minute', 60n], ['second', 1n]] as const) {
    const count = remaining / scale; remaining %= scale;
    if (count) parts.push(part(count, unit));
  }
  return parts.join(' ');
}

export function usageValue(key: string, value: string, language = 'en'): string {
  if (key.endsWith('_bytes')) return byteText(value);
  if (key === 'connection_ms') return durationText(value, language);
  return BigInt(value).toLocaleString('en-US');
}

export function usageExactValue(key: string, value: string, language = 'en'): string {
  if (key === 'connection_ms') return durationText(value, language);
  return `${value}${key.endsWith('_bytes') ? ' B' : ''}`;
}

export function localDateTime(value: number) {
  const date = new Date(value);
  const part = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${part(date.getMonth() + 1)}-${part(date.getDate())}T${part(date.getHours())}:${part(date.getMinutes())}`;
}

export function parseLocalDateTime(value: string): number | undefined {
  const time = new Date(value).getTime();
  // Reject nonexistent local times during the spring DST transition.
  return Number.isFinite(time) && localDateTime(time) === value ? time : undefined;
}
