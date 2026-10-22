import type { Page } from '@playwright/test';

export const designAccountId = '31000000-0000-4000-8000-000000000001';
export const designDeviceId = '41000000-0000-4000-8000-000000000001';

/** Synthetic, browser-local data for visual review. Never touches a real backend. */
export async function designFixture(page: Page, options: {
  theme?: string; language?: string; ordinary?: boolean; signedOut?: boolean; empty?: boolean; unavailable?: boolean;
} = {}) {
  const now = Date.now(); const hour = 3_600_000;
  await page.addInitScript(({ theme, language }) => {
    localStorage.setItem('pab-web-theme', theme); localStorage.setItem('pab-web-language', language);
  }, { theme: options.theme ?? 'light', language: options.language ?? 'zh-CN' });
  await page.routeWebSocket('**/api/web/events', () => {});
  const devices = [
    { id: designDeviceId, code: '428358431', name: 'Design Studio · Windows', online: true, system: 'windows', os_name: 'Windows', os_version: '11', architecture: 'x86_64', agent_version: '1.2.74' },
    { id: '41000000-0000-4000-8000-000000000002', code: '556688188', name: 'Build Server · Ubuntu', online: true, system: 'linux', os_name: 'Ubuntu', os_version: '24.04', architecture: 'x86_64', agent_version: '1.2.74' },
    { id: '41000000-0000-4000-8000-000000000003', code: '419438177', name: 'Creative Mac · Studio', online: true, system: 'macos', os_name: 'macOS', os_version: '15', architecture: 'aarch64', agent_version: '1.2.74' },
    { id: '41000000-0000-4000-8000-000000000004', code: '947158375', name: 'Mobile Lab · Android', online: false, system: 'android', os_name: 'Android', os_version: '13', architecture: 'aarch64', agent_version: '1.2.74' },
  ].map(device => ({ ...device, revision: 1, status: 'active', created_at: now - 30 * 24 * hour, last_online_at: now - 6 * hour, associated_user_id: designAccountId, associated_username: 'pixels-team', associated_with_me: true,
    diagnostics: { runtime_reported: true, reported_at: now - 90000, permissions: 'not_reported', compatibility: 'not_reported' } }));
  const accounts = [
    { id: designAccountId, username: 'pixels-team', server_admin: true, status: 'active', relay_limit_mbps: 50 },
    { id: '31000000-0000-4000-8000-000000000002', username: 'design-studio', server_admin: false, status: 'active', relay_limit_mbps: 20 },
    { id: '31000000-0000-4000-8000-000000000003', username: 'engineering', server_admin: false, status: 'active', relay_limit_mbps: 30 },
    { id: '31000000-0000-4000-8000-000000000004', username: 'archived-user', server_admin: false, status: 'disabled', relay_limit_mbps: 10 },
  ].map(account => ({ ...account, revision: 1, created_at: now - 30 * 24 * hour }));
  const snapshot = { summary: { configured: 3, available: 2, maintenance: 1, disabled: 0, abnormal: 0, history: 0 }, sampled_at: now, offline_seconds: 15, mode: 'cluster', semantics_version: 1,
    health: { server: 'responding', database: 'reachable', source: 'management_database_query', checked_at: now, latency_grade: 'normal' } };
  const counters = { relay_upload_bytes: '2387193856', relay_download_bytes: '7281930240', connections: '128', connection_ms: '14400000', uploaded_files: '36', downloaded_files: '81', uploaded_bytes: '1409286144', downloaded_bytes: '3489660928', failed_transfers: '2', cancelled_transfers: '1' };
  const usage = { from: now - 7 * 24 * hour, until: now, counters: {}, counters_exact: counters, incomplete: false, updated_at: now - 60000 };
  const paged = (items: unknown[], url: URL) => ({ items, total: items.length, page: Number(url.searchParams.get('page') ?? 1), page_size: 20 });
  const state = { reads: [] as string[], writes: [] as string[] };
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', '');
    if (route.request().method() !== 'GET') {
      state.writes.push(path);
      await route.fulfill({ status: 403, json: { code: 'forbidden' } }); return;
    }
    state.reads.push(path);
    let data: unknown;
    if (path === '/config') data = { registration_enabled: true, github_enabled: true, version: 'design-preview' };
    else if (path === '/session') {
      if (options.signedOut) { await route.fulfill({ status: 401, json: { code: 'unauthorized' } }); return; }
      data = { ...accounts[0], personal_tenant_id: 'design-tenant', server_admin: !options.ordinary };
    } else if (path === '/overview') {
      if (options.unavailable) { await route.fulfill({ status: 503, json: { code: 'server_error' } }); return; }
      data = options.empty ? { total: 0, online: 0, offline: 0 } : { total: 48, online: 36, offline: 12, ...(!options.ordinary && { accounts: 12, relays: 3, relay_status: snapshot }) };
    } else if (path === '/devices/diagnostics') data = { total: 48, reported: 42, not_reported: 6, sampled_at: now,
      systems: [{ value: 'windows', count: 24 }, { value: 'macos', count: 12 }, { value: 'linux', count: 8 }, { value: 'android', count: 4 }], architectures: [{ value: 'x86_64', count: 32 }, { value: 'aarch64', count: 16 }], versions: [{ value: '1.2.74', count: 42 }, { value: null, count: 6 }], versions_truncated: false };
    else if (path === '/devices') data = paged(devices, url);
    else if (path.startsWith('/devices/')) data = devices[0];
    else if (path === '/accounts') data = paged(accounts, url);
    else if (path.endsWith('/sessions') || path === '/sessions') data = paged([
      { id: '51000000-0000-4000-8000-000000000001', channel: 'web', created_at: now - 3600000, last_active_at: now - 30000, current: true },
      { id: '51000000-0000-4000-8000-000000000002', channel: 'native', created_at: now - 86400000, last_active_at: now - 600000, current: false },
    ], url);
    else if (path.startsWith('/accounts/')) data = { ...accounts[0], device_count: 4, password_enabled: true, github_enabled: true, usage: { ...usage, counters } };
    else if (path === '/traffic') data = { user_mbps: 10 };
    else if (path === '/relays') data = { ...snapshot, ...paged(['ap-east-primary', 'ap-east-secondary', 'eu-west-backup'].map((id, index) => ({
      id, region: index === 2 ? 'Frankfurt' : 'Hong Kong', url: `https://${id}.example.invalid`, online: true, health: 'healthy', configured: true, admin_state: index === 2 ? 'maintenance' : 'active', agent_version: '1.2.74', applied_policy_version: 14324, offered_policy_version: 14324, capacity_mbps: 1000, last_seen: now - 2000, sample_current_instance: true, connections_exact: String(18 + index * 6), boot_id: 'preview-boot', heartbeat_age_ms: 2000,
    })), url) };
    else if (path === '/usage/ranking') data = { ...usage, interval: 'hour', ...paged(accounts.slice(0, 3).map((account, index) => ({ subject: account.id, user_id: account.id, username: account.username, counters_exact: { ...counters, relay_download_bytes: String(3489660928 - index * 1024 ** 3) }, incomplete: false, updated_at: now - 60000 })), url) };
    else if (path === '/usage') {
      const from = Number(url.searchParams.get('from') ?? usage.from); const until = Number(url.searchParams.get('until') ?? usage.until);
      data = { ...usage, from, until, interval: url.searchParams.get('interval') ?? 'hour', timezone: 'UTC', points: Array.from({ length: 24 }, (_, index) => ({ hour_unix_ms: from + (until - from) / 24 * index, counters_exact: { relay_upload_bytes: String((Math.sin(index / 2) + 2) * 104857600 | 0), relay_download_bytes: String((Math.cos(index / 3) + 3) * 104857600 | 0), uploaded_bytes: '1048576', downloaded_bytes: '4194304' } })) };
    } else if (path === '/audit') data = { ...paged([
      { action: 'account.status_changed', resource_name: 'design-studio', resource_kind: 'account', result: 'succeeded' },
      { action: 'limits_updated', resource_name: 'engineering', resource_kind: 'account', result: 'succeeded' },
      { action: 'relay.maintenance', resource_name: 'eu-west-backup', resource_kind: 'relay', result: 'succeeded' },
      { action: 'account.password_reset', resource_name: 'archived-user', resource_kind: 'account', result: 'denied' },
    ].map((event, index) => ({ ...event, id: `61000000-0000-4000-8000-00000000000${index + 1}`, actor: 'pixels-team', actor_id: designAccountId, resource_key: event.resource_name, created_at: now - (index + 1) * 1800000, change_state: 'historical_unknown', request_id: `preview-request-${index}` })), url), snapshot_cursor: 'preview-snapshot', next_cursor: null, previous_cursor: null };
    else if (path === '/service') data = { registration_enabled: true, policy_revision: 14324, registration: { deployment_default: true, override_value: null, effective: true, source: 'deployment_default', revision: 12, effect: 'immediate' } };
    else if (path === '/github') data = { enabled: true, login: 'pixels-team', password_enabled: true };
    else if (path === '/saved-devices') data = { items: devices.map(device => ({ device_ref: { device_id: device.id }, ...device, alias: '', deleted: false })), cursor: 4, has_more: false };
    else throw new Error(`Unhandled design fixture API: ${path}`);
    await route.fulfill({ json: data });
  });
  return state;
}
