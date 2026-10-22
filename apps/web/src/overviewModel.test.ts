import { describe, expect, it } from 'vitest';
import { addValues, chartSamples, exactValue, percentChange, previousMatches, traffic } from './overviewModel';
import type { UsageResult } from './usageModel';

const result = (from = 1000, until = 2000): UsageResult => ({ from, until, interval: 'hour', timezone: 'UTC', updated_at: 1, counters: {}, counters_exact: {}, points: [] });
describe('overview statistics', () => {
  it('retains exact large integers and never turns missing data into measured zero', () => {
    expect(exactValue(undefined, 'connections')).toBeUndefined();
    expect(exactValue(result(), 'connections')).toBeUndefined();
    expect(exactValue({ updated_at: null, counters_exact: { connections: '7' } }, 'connections')).toBeUndefined();
    expect(exactValue({ counters_exact: { connections: '9007199254740993' } }, 'connections')).toBe(9007199254740993n);
    expect(exactValue({ counters: { connections: 9007199254740992 } }, 'connections')).toBeUndefined();
    expect(addValues(1n, undefined)).toBeUndefined();
  });
  it('only reports P2P totals when explicitly collected', () => {
    const data = { ...result(), counters_exact: { p2p_upload_bytes: '10', p2p_download_bytes: '20' } };
    expect(traffic(data, 'p2p')).toBeUndefined();
    expect(traffic({ ...data, p2p_reported: true }, 'p2p')).toBe(30n);
  });
  it('matches comparison windows before comparing asynchronously returned totals', () => {
    expect(previousMatches(result(), result(0, 1000))).toEqual(result(0, 1000));
    expect(previousMatches(result(), result(-1000, 0))).toBeUndefined();
    expect(percentChange(3n, 2n)).toBe('+50%');
    expect(percentChange(0n, 0n)).toBeUndefined();
  });
  it('preserves missing P2P chart points and exact zero reports as distinct values', () => {
    const points = [{ hour_unix_ms: 0, counters_exact: { p2p_download_bytes: '0' } }, { hour_unix_ms: 3600000, counters_exact: { p2p_download_bytes: '0' }, p2p_reported: true }];
    expect(chartSamples(points, ['p2p_download_bytes'], false).map(point => point.values[0])).toEqual([undefined, 0n]);
  });
});
