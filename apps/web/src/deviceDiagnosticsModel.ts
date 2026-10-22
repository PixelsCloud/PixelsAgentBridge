const keys = ['q', 'page', 'page_size', 'system', 'status', 'user', 'architecture', 'version', 'version_state'];
export const deviceFilterKeys = ['q', 'status', 'system', 'architecture', 'version_state', 'version'] as const;
export type DeviceFilterValues = Record<typeof deviceFilterKeys[number], string>;
export function deviceFilterValues(params: URLSearchParams): DeviceFilterValues {
  return Object.fromEntries(deviceFilterKeys.map(key => [key, params.get(key) ?? ''])) as DeviceFilterValues;
}
export function applyDeviceFilters(params: URLSearchParams, values: DeviceFilterValues) {
  const next = new URLSearchParams(params); next.delete('page');
  for (const key of deviceFilterKeys) {
    const value = (values[key] ?? '').trim();
    value ? next.set(key, value) : next.delete(key);
  }
  if (next.get('version_state') === 'unknown') next.delete('version');
  else if (next.has('version')) next.delete('version_state');
  return next;
}
export function deviceQuery(params: URLSearchParams, admin: boolean, diagnostics = false) {
  const query = new URLSearchParams();
  for (const key of keys) {
    if (diagnostics && (key === 'page' || key === 'page_size')) continue;
    const value = params.get(key); if (value) query.set(key, value);
  }
  query.set('scope', admin ? 'all' : 'mine'); return query;
}
export function deviceFilter(params: URLSearchParams, key: string, value?: string) {
  const next = new URLSearchParams(params); next.delete('page');
  value ? next.set(key, value) : next.delete(key);
  if (key === 'version' && value) next.delete('version_state');
  if (key === 'version_state' && value === 'unknown') next.delete('version');
  return next;
}
