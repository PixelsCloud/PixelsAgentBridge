import { useEffect, useState } from 'react';
import { useResource, useLiveRefresh } from './useResource';
import { usageQuery, usageWindow, type UsageResult } from './usageModel';
import type { Devices, Diagnostics, Nodes, OverviewData, Period, Ranking, RankingMetric } from './overviewModel';

export function useOverviewData(period: Period, sort: RankingMetric, devicePage: number, search: string, attention: boolean, revision: number) {
  const params = new URLSearchParams({ period });
  const window = usageWindow(params);
  const query = usageQuery(params, window, true);
  const previousWindow = { ...window, from: window.from - (window.until - window.from), until: window.from };
  const overview = useResource<OverviewData>('/overview?scope=all');
  const diagnostics = useResource<Diagnostics>('/devices/diagnostics?scope=all');
  const usage = useResource<UsageResult>(`/usage?${query}`, { retainPreviousData: true });
  const previous = useResource<UsageResult>(period === 'lifetime' ? null : `/usage?${usageQuery(params, previousWindow, true)}`, { retainPreviousData: true });
  const monthParams = new URLSearchParams({ period: 'month' });
  const month = useResource<UsageResult>(period === 'lifetime' ? `/usage?${usageQuery(monthParams, usageWindow(monthParams), true)}` : null);
  // Follow the displayed summary bounds, so period changes cannot mix a new
  // ranking with totals that are still awaiting their replacement response.
  const rankingQuery = new URLSearchParams(query);
  if (usage.data) {
    rankingQuery.set('interval', usage.data.interval);
    if (usage.data.from != null && usage.data.until != null) { rankingQuery.set('from', String(usage.data.from)); rankingQuery.set('until', String(usage.data.until)); }
    else { rankingQuery.delete('from'); rankingQuery.delete('until'); }
  }
  const ranking = useResource<Ranking>(usage.error ? null : `/usage/ranking?${rankingQuery}&sort=${sort}&order=desc&page_size=5`, { retainPreviousData: true });
  const devicesQuery = new URLSearchParams({ scope: 'all', page: String(devicePage), page_size: '4', sort: 'reported', owner_search: 'true' });
  if (search) devicesQuery.set('q', search);
  if (attention) devicesQuery.set('attention', 'true');
  const devices = useResource<Devices>(`/devices?${devicesQuery}`);
  const nodes = useResource<Nodes>('/relays?page_size=20');
  useLiveRefresh(() => {
    overview.refresh(); diagnostics.refresh(); usage.refresh(); previous.refresh(); month.refresh(); ranking.refresh(); devices.refresh();
  }, revision);
  useEffect(() => {
    const refreshVisible = () => { if (document.visibilityState !== 'hidden') nodes.refresh(); };
    const interval = setInterval(refreshVisible, 5000);
    document.addEventListener('visibilitychange', refreshVisible);
    return () => { clearInterval(interval); document.removeEventListener('visibilitychange', refreshVisible); };
  }, [nodes.refresh]);
  const refresh = () => { overview.refresh(); diagnostics.refresh(); usage.refresh(); previous.refresh(); month.refresh(); ranking.refresh(); devices.refresh(); nodes.refresh(); };
  return { overview, diagnostics, usage, previous, month, ranking, devices, nodes, query, refresh };
}

export function useDebouncedSearch(value: string) {
  const [query, setQuery] = useState(value);
  useEffect(() => { const timer = setTimeout(() => setQuery(value.trim()), 250); return () => clearTimeout(timer); }, [value]);
  return query;
}
