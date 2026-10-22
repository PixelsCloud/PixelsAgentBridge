import { expect, it } from 'vitest';
import { applyDeviceFilters, deviceFilter, deviceFilterValues, deviceQuery } from './deviceDiagnosticsModel';
it('uses the same scoped filters for rows and distributions without pagination or arbitrary fields', () => {
  const params = new URLSearchParams('scope=all&user=id&version=1.2.3&system=android&architecture=aarch64&status=offline&q=device&page=3&page_size=50&password=secret');
  const rows = deviceQuery(params, false); const stats = deviceQuery(params, false, true);
  expect(rows.get('scope')).toBe('mine'); expect(stats.get('scope')).toBe('mine');
  expect(stats.has('page')).toBe(false); expect(rows.get('page')).toBe('3'); expect(stats.get('version')).toBe('1.2.3');
  expect(rows.has('password')).toBe(false); rows.delete('page'); rows.delete('page_size'); expect(rows.toString()).toBe(stats.toString());
});
it('applies all draft fields together without losing text and preserves account and page size', () => {
  const current = new URLSearchParams('user=account&page=4&page_size=50&system=windows');
  const values = { ...deviceFilterValues(current), q: ' phone ', system: 'android', architecture: 'aarch64', version: ' 1.2.48 ' };
  const next = applyDeviceFilters(current, values);
  expect(next.get('q')).toBe('phone'); expect(next.get('system')).toBe('android'); expect(next.get('version')).toBe('1.2.48');
  expect(next.get('architecture')).toBe('aarch64'); expect(next.get('user')).toBe('account'); expect(next.get('page_size')).toBe('50'); expect(next.has('page')).toBe(false);
  expect(current.get('page')).toBe('4');
});
it('unknown versions cannot submit an exact version, and removing an empty exact version preserves unknown state', () => {
  const current = new URLSearchParams('version_state=unknown&user=account');
  const next = applyDeviceFilters(current, { ...deviceFilterValues(current), version: 'obsolete' });
  expect(next.has('version')).toBe(false); expect(next.get('version_state')).toBe('unknown');
  expect(deviceFilter(current, 'version').get('version_state')).toBe('unknown');
});
it('drops incompatible version filters and resets the page while preserving account scope', () => {
  const current = new URLSearchParams('version=old&page=3&user=account');
  const unknown = deviceFilter(current, 'version_state', 'unknown'); expect(unknown.has('version')).toBe(false); expect(unknown.has('page')).toBe(false);
  const exact = deviceFilter(unknown, 'version', 'new'); expect(exact.has('version_state')).toBe(false); expect(exact.get('user')).toBe('account');
  expect(current.get('page')).toBe('3');
});
