import { useSearchParams } from 'react-router-dom';
import { useLiveRefresh, useResource } from './useResource';
import type { RelaySnapshot } from './RelaySummary';

export interface Page<T> extends Partial<RelaySnapshot> { items: T[]; total: number; page: number; page_size: number }

export function usePage<T>(path: string, liveRevision: number, defaultPageSize?: number) {
  const [params, setParams] = useSearchParams();
  const query = new URLSearchParams();
  for (const key of ['q', 'page', 'page_size']) { const value = params.get(key); if (value) query.set(key, value); }
  if (defaultPageSize !== undefined && !query.has('page_size')) query.set('page_size', String(defaultPageSize));
  if (path === '/relays' && params.get('status')) query.set('status', params.get('status')!);
  const resource = useResource<Page<T>>(`${path}?${query}`);
  useLiveRefresh(resource.refresh, liveRevision);
  const search = (value: string) => { const next = new URLSearchParams(params); value ? next.set('q', value) : next.delete('q'); next.delete('page'); setParams(next); };
  const paginate = (page: number, size: number) => { const next = new URLSearchParams(params); next.set('page', String(page)); next.set('page_size', String(size)); setParams(next); };
  return { ...resource, search, paginate, query: params.get('q') ?? '' };
}
