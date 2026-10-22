import { expect, test, type Page } from '@playwright/test';
import { designFixture } from './support/designFixture';

async function fixture(page: Page, language = 'en', theme = 'light', unreported = false) {
  const state = await designFixture(page, { language, theme });
  const now = Date.now(); const hour = 3600000;
  const devices = Array.from({ length: 9 }, (_, index) => ({
    id: `41000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`, code: `40000000${index}`, name: `Device ${index + 1}`,
    associated_username: index % 2 ? 'company' : 'home', online: index < 8,
    system: index === 8 ? 'android' : index === 7 ? 'macos' : index > 4 ? 'linux' : 'windows', architecture: index > 6 ? 'aarch64' : 'x86_64',
    agent_version: '1.3.15', diagnostics: { runtime_reported: true, reported_at: now - index * hour },
  }));
  const snapshot = { summary: { configured: 1, available: 1, maintenance: 0, disabled: 0, abnormal: 0, history: 0 }, sampled_at: now,
    health: { server: 'responding', database: 'reachable' } };
  await page.route('**/api/web/overview?*', route => route.fulfill({ json: { total: 9, online: 8, offline: 1, accounts: 5, account_summary: { enabled: 4, disabled: 1, administrators: 1 }, relay_status: snapshot } }));
  await page.route('**/api/web/devices/diagnostics?*', route => route.fulfill({ json: { total: 9, reported: 9, not_reported: 0, sampled_at: now,
    systems: [{ value: 'windows', count: 5 }, { value: 'linux', count: 2 }, { value: 'macos', count: 1 }, { value: 'android', count: 1 }],
    architectures: [{ value: 'x86_64', count: 7 }, { value: 'aarch64', count: 2 }], versions: [{ value: '1.3.15', count: 9 }] } }));
  const deviceReads: URL[] = [];
  await page.route('**/api/web/devices?*', route => {
    const url = new URL(route.request().url()); deviceReads.push(url);
    let rows = [...devices];
    if (url.searchParams.get('attention') === 'true') rows = rows.filter(row => !row.online);
    const search = url.searchParams.get('q')?.toLowerCase();
    if (search) rows = rows.filter(row => `${row.name} ${row.associated_username}`.toLowerCase().includes(search));
    const page = Number(url.searchParams.get('page')); const size = Number(url.searchParams.get('page_size'));
    return route.fulfill({ json: { items: rows.slice((page - 1) * size, page * size), total: rows.length, page, page_size: size } });
  });
  await page.route('**/api/web/relays?*', route => route.fulfill({ json: { ...snapshot, total: 1, items: [{ id: 'cn-primary', online: true, configured: true, sample_current_instance: true, agent_version: '1.3.15', capacity_mode: 'host', capacity_mbps: 0,
    telemetry_current: !unreported, connections_exact: '13', uploaded_bytes_exact: '1048576', downloaded_bytes_exact: '2097152', applied_policy_version: 100, offered_policy_version: 100, heartbeat_age_ms: 1000 }] } }));
  const usageReads: URL[] = [];
  await page.route('**/api/web/usage?*', route => {
    const url = new URL(route.request().url()); usageReads.push(url);
    const lifetime = url.searchParams.get('interval') === 'lifetime';
    const from = Number(url.searchParams.get('from')); const until = Number(url.searchParams.get('until'));
    const large = lifetime || until - from > 15 * 86400000;
    return route.fulfill({ json: { from: lifetime ? null : from, until: lifetime ? null : until, interval: lifetime ? 'lifetime' : 'hour', updated_at: now,
      counters_exact: { relay_upload_bytes: large ? '2097152' : '1048576', relay_download_bytes: '1048576', p2p_upload_bytes: '2097152', p2p_download_bytes: '4194304', connection_ms: '107703661', connections: '3907', uploaded_files: '12', downloaded_files: '7', uploaded_bytes: '1048576', downloaded_bytes: '2097152', failed_transfers: '1', cancelled_transfers: '0' }, p2p_reported: !unreported,
      points: lifetime ? [] : Array.from({ length: 3 }, (_, index) => ({ hour_unix_ms: from + index * hour, p2p_reported: !unreported, counters_exact: { relay_upload_bytes: String((index + 1) * 1048576), relay_download_bytes: '1048576', p2p_upload_bytes: '2097152', p2p_download_bytes: '4194304', connection_ms: '3600000' } })) } });
  });
  const rankingReads: URL[] = [];
  await page.route('**/api/web/usage/ranking?*', route => {
    const url = new URL(route.request().url()); rankingReads.push(url);
    return route.fulfill({ json: { from: url.searchParams.get('from') === null ? null : Number(url.searchParams.get('from')), until: url.searchParams.get('until') === null ? null : Number(url.searchParams.get('until')), interval: url.searchParams.get('interval'), items: [{ subject: 'home', user_id: '31000000-0000-4000-8000-000000000001', username: url.searchParams.get('sort') === 'connection_ms' ? 'duration-user' : 'home', p2p_reported: !unreported, updated_at: now, counters_exact: { relay_download_bytes: '1048576', p2p_download_bytes: '4194304', connection_ms: '107703661' } }] } });
  });
  return { ...state, devices, deviceReads, usageReads, rankingReads };
}

test('overview uses bounded aggregate APIs, real telemetry and one connection summary', async ({ page }) => {
  const state = await fixture(page); await page.goto('/');
  await expect(page.locator('.admin-overview .metric-value')).toHaveText('9');
  await expect(page.locator('.metric-number').first()).toHaveText('2 MiB');
  await expect(page.locator('.node-live strong').first()).toHaveText('13');
  await expect(page.locator('[data-platform="android"]')).toContainText('1');
  await expect(page.locator('.rank-name')).toHaveText('home');
  expect(state.reads.filter(path => path === '/accounts')).toHaveLength(0);
  expect(state.writes).toEqual([]);
  await expect(page.locator('.quick-access-card, .connectivity-card')).toHaveCount(0);
  await expect(page.locator('.connection-values')).toHaveCount(1);
});

test('period and ranking changes preserve global counts and the existing metric elements', async ({ page }) => {
  const state = await fixture(page); await page.goto('/');
  await expect(page.locator('.metric-number').first()).toHaveText('2 MiB');
  const metric = await page.locator('.metric-number').first().elementHandle();
  const fleet = await page.locator('.fleet-overview').innerText();
  await page.getByRole('button', { name: 'This month', exact: true }).click();
  await expect(page.locator('.metric-number').first()).toHaveText('3 MiB');
  expect(await metric!.evaluate(node => node.isConnected)).toBe(true);
  expect(await page.locator('.fleet-overview').innerText()).toBe(fleet);
  await page.getByRole('combobox', { name: 'Ranking metric', exact: true }).selectOption('connection_ms');
  await expect(page.locator('.rank-name')).toHaveText('duration-user');
  expect(state.rankingReads.at(-1)?.searchParams.get('sort')).toBe('connection_ms');
  await page.getByRole('button', { name: 'Lifetime', exact: true }).click();
  await expect.poll(() => state.usageReads.some(url => url.searchParams.get('interval') === 'lifetime')).toBe(true);
  await expect(page.locator('.chart-meta')).toContainText('This month’s trend');
});

test('device search, attention and pagination use server filters without changing platform totals', async ({ page }) => {
  const state = await fixture(page); await page.goto('/');
  await expect(page.locator('.device-name')).toHaveCount(4);
  await page.getByRole('button', { name: 'Next devices', exact: true }).click();
  await expect.poll(() => state.deviceReads.at(-1)?.searchParams.get('page')).toBe('2');
  await expect(page.locator('.device-name').first()).toContainText('Device 5');
  await page.getByRole('searchbox', { name: 'Search device or user', exact: true }).fill('company');
  await expect.poll(() => state.deviceReads.at(-1)?.searchParams.get('q')).toBe('company');
  expect(state.deviceReads.at(-1)?.searchParams.get('owner_search')).toBe('true');
  expect(state.deviceReads.at(-1)?.searchParams.get('page')).toBe('1');
  await page.getByRole('searchbox', { name: 'Search device or user', exact: true }).fill('');
  await page.getByRole('button', { name: 'Needs attention', exact: true }).click();
  await expect(page.locator('.device-name')).toHaveCount(1);
  await expect(page.locator('.device-name')).toContainText('Device 9');
  await expect(page.locator('.metric-value')).toHaveText('9');
  await expect(page.locator('[data-platform="windows"] strong')).toHaveText('5');
});

test('missing P2P reports and stale Relay counters never appear as measured zero', async ({ page }) => {
  await fixture(page, 'en', 'light', true); await page.goto('/');
  await expect(page.locator('.metrics .metric').nth(1)).toContainText('Not reported');
  await expect(page.locator('.node-live strong').first()).toHaveText('—');
  await expect(page.locator('.node-traffic strong')).toHaveText(['—', '—']);
  await expect(page.locator('.rank-name')).toHaveText('home');
  await expect(page.locator('.ranking-table tbody td').nth(2)).toHaveText('—');
});

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 768, 1440, 1920]) {
  test(`compact overview layout ${language} ${theme} ${width}`, async ({ page }) => {
    await fixture(page, language, theme); await page.setViewportSize({ width, height: 1100 });
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto('/'); await expect(page.locator('.metric-value')).toHaveText('9');
    await expect(page.locator('.rank-name')).toHaveText('home');
    await expect(page.locator('.console-footer, .overview-scope-note')).toHaveCount(0);
    const smallText = await page.locator('.admin-overview .metric-heading, .admin-overview .metric-description, .admin-overview .metric-footer, .admin-overview .duplex, .admin-overview .chart-axis, .admin-overview .data-table td, .admin-overview .data-table th, .admin-overview .device-secondary, .admin-overview .device-report, .admin-overview .platform-name, .admin-overview .service-status, .admin-overview .fleet-details, .admin-overview .node-title small, .admin-overview .transfer-strip small').evaluateAll(nodes => nodes.filter(node => Number.parseFloat(getComputedStyle(node).fontSize) < 13).map(node => ({ name: node.className, size: getComputedStyle(node).fontSize })));
    expect(smallText).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 1);
    const overflow = await page.locator('.admin-overview .metric, .admin-overview .fleet-card, .admin-overview .panel').evaluateAll(nodes => nodes.filter(node => node.scrollWidth > node.clientWidth + 2).map(node => ({ name: node.className, scroll: node.scrollWidth, width: node.clientWidth })));
    expect(overflow).toEqual([]);
    if (width >= 1440) expect((await page.locator('.metrics').boundingBox())!.height).toBeLessThan(160);
    await page.getByRole('group', { name: language === 'en' ? 'Platforms' : '平台分布', exact: true }).getByRole('button', { name: language === 'en' ? 'Arch' : '架构', exact: true }).click();
    await expect(page.locator('.architecture-item')).toHaveCount(2);
    await page.getByRole('group', { name: language === 'en' ? 'Platforms' : '平台分布', exact: true }).getByRole('button', { name: language === 'en' ? 'Version' : '版本', exact: true }).click();
    await expect(page.locator('.version-row')).toContainText('1.3.15');
    expect(errors).toEqual([]);
  });
}
