import { describe, expect, it } from 'vitest';
import { auditFilters, auditQuery } from './auditModel';

describe('audit URL contract', () => {
  it('preserves all filters and the frozen snapshot, excluding arbitrary input', () => {
    const params = new URLSearchParams('q=snapshot&from=1000&until=2000&actor=abc&action=account.updated&result=failed&resource_kind=device&resource=xyz&page_size=50&cursor=frozen&password=secret&page=2');
    const query = auditQuery(params);
    expect(query.get('cursor')).toBe('frozen');
    expect(query.get('q')).toBe('snapshot');
    expect(query.get('from')).toBe('1000');
    expect(query.get('resource_kind')).toBe('device');
    expect(query.has('password')).toBe(false);
    expect(query.has('page')).toBe(false);
    expect([...query]).toHaveLength(10);
  });
  it('discards the previous snapshot whenever a filter or page size changes', () => {
    const current = new URLSearchParams('q=old&actor=actor&cursor=old-snapshot&page=3');
    const next = auditFilters(current, { q: 'new', page_size: '100', password: 'never forwarded' });
    expect(Object.fromEntries(next)).toEqual({ q: 'new', actor: 'actor', page_size: '100' });
    expect(current.get('cursor')).toBe('old-snapshot');
  });
  it('distinguishes clearing a filter from leaving it unchanged', () => {
    expect(Object.fromEntries(auditFilters(new URLSearchParams('q=old&result=denied&cursor=old'), { q: '' }))).toEqual({ result: 'denied' });
    expect(auditFilters(new URLSearchParams(), { q: '', start: 'local-only', end: 'local-only' }).toString()).toBe('');
  });
});
