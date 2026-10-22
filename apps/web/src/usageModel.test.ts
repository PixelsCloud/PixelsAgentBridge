import { describe, expect, it } from 'vitest';
import { byteText, counter, DAY, HOUR, durationText, localDateTime, parseLocalDateTime, usageExactValue, usageValue, usageQuery, usageWindow } from './usageModel';

describe('usage precision and query boundaries', () => {
  it('keeps counters beyond JS and u64 exact and rejects unsafe legacy numbers', () => {
    expect(counter({ uploaded_bytes: '36893488147419103230' }, undefined, 'uploaded_bytes')).toBe('36893488147419103230');
    expect(counter(undefined, { uploaded_bytes: 9007199254740993 }, 'uploaded_bytes')).toBeUndefined();
    expect(counter(undefined, { uploaded_bytes: 4096 }, 'uploaded_bytes')).toBe('4096');
    expect(byteText('4096')).toBe('4 KiB');
    expect(byteText('36893488147419103230')).toBe('32 EiB');
  });
  it('rounds byte sizes consistently without converting exact counters to floating point', () => {
    for (const [value, text] of [['0', '0 B'], ['1023', '1023 B'], ['1024', '1 KiB'], ['1809', '1.77 KiB'], ['403812076', '385.11 MiB'], ['393477624', '375.25 MiB'], ['1048575', '1 MiB'], ['9007199254740993', '8 PiB']]) expect(byteText(value)).toBe(text);
    expect(usageValue('connections', '9007199254740993')).toBe('9,007,199,254,740,993');
    expect(usageValue('connection_ms', '12345')).toBe('12 sec');
    expect(usageExactValue('uploaded_bytes', '9007199254740993')).toBe('9007199254740993 B');
    expect(usageExactValue('connections', '12345')).toBe('12345');
  });
  it('formats cumulative durations in localized days, hours, minutes and seconds', () => {
    expect(durationText('107703661', 'zh-CN')).toBe('1天 5小时 55分钟 3秒');
    expect(durationText('10410259')).toBe('2 hr 53 min 30 sec');
    expect(durationText('0', 'zh-CN')).toBe('0秒');
    expect(durationText('1', 'zh-CN')).toBe('< 1秒');
    expect(durationText('3600000')).toBe('1 hr');
    expect(usageExactValue('connection_ms', '107703661', 'zh-CN')).toBe('1天 5小时 55分钟 3秒');
  });
  it('automatically uses daily history and refuses pruned hourly or daily windows', () => {
    const now = new Date('2026-10-20T04:21:00Z');
    expect(usageWindow(new URLSearchParams('period=180'), now).interval).toBe('day');
    expect(usageWindow(new URLSearchParams('period=180&interval=hour'), now).error).toBe('usage_range_unavailable');
    expect(usageWindow(new URLSearchParams('period=730'), now).error).toBeUndefined();
    expect(usageWindow(new URLSearchParams('period=731'), now).error).toBe('usage_range_unavailable');
    expect(usageWindow(new URLSearchParams('period=custom&from=NaN&until=1'), now).error).toBe('invalid_input');
  });
  it('shares the same half-open selection with summary, ranking and export and removes personal scope overrides', () => {
    const params = new URLSearchParams('period=7&user=other&population=users');
    const window = usageWindow(params, new Date('2026-10-20T04:21:00Z'));
    const query = usageQuery(params, window, true);
    expect(Number(query.get('until')) - Number(query.get('from'))).toBe(7 * DAY);
    expect(query.get('user')).toBe('other'); expect(query.get('population')).toBe('users');
    const personal = usageQuery(params, window, false);
    expect(personal.get('scope')).toBe('mine'); expect(personal.has('user')).toBe(false); expect(personal.has('population')).toBe(false);
    const lifetime = usageQuery(params, usageWindow(new URLSearchParams('period=lifetime')), true);
    expect(lifetime.has('from')).toBe(false); expect(lifetime.has('until')).toBe(false);
  });
  for (const zone of ['America/New_York', 'Asia/Kathmandu', 'Australia/Adelaide']) {
    it(`aligns local day/month inputs to explicit UTC buckets in ${zone}`, () => {
      const previous = process.env.TZ; process.env.TZ = zone;
      try {
        for (const timestamp of ['2026-03-08T16:00:00Z', '2026-11-01T16:00:00Z', '2026-08-31T16:00:00Z']) {
          const now = new Date(timestamp);
          for (const period of ['today', 'month']) {
            const window = usageWindow(new URLSearchParams(`period=${period}`), now);
            const midnight = new Date(now.getFullYear(), now.getMonth(), period === 'month' ? 1 : now.getDate()).getTime();
            expect(window.from).toBe(Math.floor(midnight / HOUR) * HOUR);
            expect(window.until % HOUR).toBe(0); expect(window.until).toBeGreaterThan(now.getTime());
          }
        }
      } finally { if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous; }
    });
  }
  it('rejects a nonexistent spring DST time and round-trips valid local input', () => {
    const previous = process.env.TZ; process.env.TZ = 'America/New_York';
    try {
      expect(parseLocalDateTime('2026-03-08T02:30')).toBeUndefined();
      expect(localDateTime(parseLocalDateTime('2026-11-01T01:30')!)).toBe('2026-11-01T01:30');
    } finally { if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous; }
  });
});
