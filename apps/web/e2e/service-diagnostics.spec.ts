import { screenshotPath } from './support/isolatedFixture';
import { expect, test, type Page } from '@playwright/test';
const id = '20545b3a-e6f6-4b91-a122-9072055487ea';
const owner = '30545b3a-e6f6-4b91-a122-9072055487ea';
async function fixture(page: Page, options: { language?: string; theme?: string; fail?: 'conflict' | 'lost'; ordinary?: boolean } = {}) {
  const state = { override: null as boolean | null, revision: 1, writes: [] as { revision: number; registration_override: boolean | null }[], rows: [] as URL[], stats: [] as URL[], failed: false };
  const device = { id, code: '123456789', name: 'Diagnostic phone', revision: 1, system: 'android', os_name: 'Android', os_version: '9', architecture: 'aarch64', agent_version: '1.2.48', created_at: 1800000000000, online: false, last_online_at: null, associated_user_id: owner, associated_username: 'Diagnostic owner', diagnostics: { runtime_reported: true, reported_at: 1800000000000, permissions: 'not_reported', compatibility: 'not_reported' } };
  const devices = [device, { ...device, id: '40545b3a-e6f6-4b91-a122-9072055487ea', code: '987654321', name: 'Legacy fixture', system: null, os_name: null, os_version: null, architecture: null, agent_version: null, diagnostics: { runtime_reported: false, reported_at: null, permissions: 'not_reported', compatibility: 'not_reported' } }];
  const filtered = (url: URL) => devices.filter(item => {
    const p = url.searchParams; const q = (p.get('q') ?? '').toLowerCase();
    return (!q || item.name.toLowerCase().includes(q) || item.code.includes(q.replaceAll(' ', '')))
      && (!p.get('system') || (item.system ?? 'unknown') === p.get('system'))
      && (!p.get('architecture') || (item.architecture ?? 'unknown') === p.get('architecture'))
      && (!p.get('version') || item.agent_version === p.get('version'))
      && (!p.get('version_state') || (p.get('version_state') === 'reported') === !!item.agent_version)
      && (!p.get('status') || (p.get('status') === 'online') === item.online);
  });
  await page.addInitScript(({ language, theme }) => { localStorage.setItem('pab-web-language', language); localStorage.setItem('pab-web-theme', theme); }, { language: options.language ?? 'en', theme: options.theme ?? 'light' });
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', ''); let data: unknown;
    const registration = () => ({ deployment_default: true, override_value: state.override, effective: state.override ?? true, source: state.override === null ? 'deployment_default' : 'runtime_override', revision: state.revision, effect: 'immediate' });
    if (path === '/config') data = { registration_enabled: true, version: 'fixture' };
    else if (path === '/session') data = { id: owner, username: 'Diagnostic owner', server_admin: !options.ordinary };
    else if (path === '/service') {
      if (route.request().method() === 'PATCH') {
        const input = route.request().postDataJSON(); state.writes.push(input);
        state.revision++; state.override = input.registration_override;
        if (options.fail && !state.failed) { state.failed = true; if (options.fail === 'lost') await route.abort('failed'); else await route.fulfill({ status: 409, json: { code: 'conflict' } }); return; }
      }
      data = { registration_enabled: state.override ?? true, registration: registration(), policy_revision: 123, session_expires: false };
    } else if (path === '/devices/diagnostics') {
      state.stats.push(url); const rows = filtered(url);
      const group = (key: 'system' | 'architecture' | 'agent_version') => [...new Set(rows.map(item => item[key]))].map(value => ({ value, count: rows.filter(item => item[key] === value).length }));
      const reported = rows.filter(item => item.diagnostics.runtime_reported).length;
      data = { total: rows.length, online: rows.filter(item => item.online).length, offline: rows.filter(item => !item.online).length, reported, not_reported: rows.length - reported, sampled_at: 1800000000000, systems: group('system'), architectures: group('architecture'), versions: group('agent_version'), versions_truncated: false };
    } else if (path === '/devices') { state.rows.push(url); const rows = filtered(url); const page = Number(url.searchParams.get('page') ?? 1); const size = Number(url.searchParams.get('page_size') ?? 20); data = { items: rows.slice((page-1)*size, page*size), total: rows.length, page, page_size: size }; }
    else if (path === `/devices/${id}`) data = device;
    else if (path === `/accounts/${owner}`) data = { id: owner, username: 'Diagnostic owner', status: 'active', revision: 1, server_admin: false, local_password: true, device_count: 1, usage: {} };
    else if (path.endsWith('/sessions')) data = { items: [], total: 0 };
    else throw new Error(`Unexpected B6 fixture request: ${path}`);
    await route.fulfill({ json: data });
  });
  return state;
}

test('registration override and restore send explicit null and the latest revision', async ({ page }) => {
  const state = await fixture(page); await page.goto('/settings');
  const selector = page.getByRole('combobox', { name: 'Runtime override', exact: true }); await expect(selector).toBeVisible();
  await selector.selectOption('disabled');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(1); expect(state.writes[0]).toEqual({ revision: 1, registration_override: false });
  await page.getByRole('button', { name: 'Restore deployment default', exact: true }).click();
  await expect.poll(() => state.writes.length).toBe(2); expect(state.writes[1]).toEqual({ revision: 2, registration_override: null });
  await expect(page.getByRole('button', { name: 'Restore deployment default', exact: true })).toBeDisabled();
});
for (const fail of ['conflict', 'lost'] as const) test(`policy ${fail} response never causes an automatic retry or stale overwrite`, async ({ page }) => {
  const state = await fixture(page, { fail }); await page.goto('/settings');
  await page.getByRole('combobox', { name: 'Runtime override', exact: true }).selectOption('disabled');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Another administrator changed this policy. Refresh and review before submitting.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled(); expect(state.writes).toHaveLength(1);
  if (fail === 'lost') await expect(page.getByText('The commit result is unconfirmed. Refresh and check the value and revision before acting again.', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await page.getByRole('combobox', { name: 'Runtime override', exact: true }).selectOption('enabled');
  await page.getByRole('button', { name: 'Save', exact: true }).click(); await expect.poll(() => state.writes.length).toBe(2); expect(state.writes[1].revision).toBe(2);
});

test('version dropdown and unknown filters share URL scope across reload', async ({ page }) => {
  const state = await fixture(page); await page.goto(`/devices?user=${owner}&page=3`);
  await page.getByRole('combobox', { name: 'Agent version', exact: true }).selectOption('1.2.48');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page).toHaveURL(/version=1.2.48/); expect(new URL(page.url()).searchParams.has('page')).toBe(false);
  await expect.poll(() => state.rows.at(-1)?.searchParams.get('version')).toBe('1.2.48');
  await expect(page.locator('.diagnostic-metric-total>strong')).toHaveText('2');
  expect(state.stats.every(url => !url.searchParams.has('version'))).toBe(true);
  await page.getByRole('combobox', { name: 'Version report status', exact: true }).selectOption('unknown');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page).toHaveURL(/version_state=unknown/); expect(new URL(page.url()).searchParams.has('version')).toBe(false);
  await page.reload(); await expect.poll(() => state.rows.at(-1)?.searchParams.get('version_state')).toBe('unknown');
  await expect(page.getByText('Diagnostic phone', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await page.getByRole('button', { name: /123 456 789/ }).click();
  await expect(page.getByText('Unknown: no permission summary reported', { exact: true })).toBeVisible();
  await expect(page.getByText('Unknown: no compatibility conclusion reported', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Diagnostic owner', exact: true }).click(); await expect(page).toHaveURL(`/accounts/${owner}`);
});

test('combined draft filters apply once, change real rows, survive refresh and clear within account scope', async ({ page }) => {
  const state = await fixture(page); await page.goto(`/devices?user=${owner}&page=3&page_size=50`);
  const search = page.getByPlaceholder('Search name or device code');
  await search.fill('Diagnostic');
  const choose = async (name: string, label: string) => { await page.getByRole('combobox', { name, exact: true }).selectOption({ label }); };
  await choose('System', 'Android'); await choose('Architecture', 'aarch64'); await choose('Online', 'Offline');
  await page.getByRole('combobox', { name: 'Agent version', exact: true }).selectOption('1.2.48');
  await expect(search).toHaveValue('Diagnostic'); expect(new URL(page.url()).searchParams.has('q')).toBe(false);
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page.getByText('Diagnostic phone', { exact: true })).toBeVisible();
  await expect(page.getByText('Legacy fixture', { exact: true })).toHaveCount(0);
  await expect(page.locator('.diagnostic-metric-total>strong')).toHaveText('2');
  for (const [key, value] of Object.entries({ q: 'Diagnostic', system: 'android', architecture: 'aarch64', status: 'offline', version: '1.2.48', user: owner })) expect(state.rows.at(-1)?.searchParams.get(key)).toBe(value);
  expect(state.stats.every(url => !['q', 'system', 'architecture', 'status', 'version', 'version_state', 'page', 'page_size'].some(key => url.searchParams.has(key)))).toBe(true);
  expect(new URL(page.url()).searchParams.has('page')).toBe(false); expect(new URL(page.url()).searchParams.get('page_size')).toBe('50');
  await expect(page.getByRole('group', { name: 'Active filters' })).toContainText('Android');
  const rowReads = state.rows.length, statReads = state.stats.length;
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect.poll(() => state.rows.length).toBeGreaterThan(rowReads); await expect.poll(() => state.stats.length).toBeGreaterThan(statReads);
  await page.reload(); await expect(search).toHaveValue('Diagnostic');
  await expect(page.getByRole('combobox', { name: 'Agent version', exact: true })).toHaveValue('1.2.48');
  await page.locator('.device-active-filters .ant-tag').filter({ hasText: 'Agent version:' }).locator('.ant-tag-close-icon').click();
  await expect(page.getByRole('combobox', { name: 'Agent version', exact: true })).toHaveValue('');
  expect(new URL(page.url()).searchParams.get('system')).toBe('android');
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await expect(page.getByText('Legacy fixture', { exact: true })).toBeVisible();
  expect(new URL(page.url()).searchParams.get('user')).toBe(owner);
  expect(state.writes).toEqual([]);
});

test('filtered pagination preserves conditions while dropdown selection resets the page', async ({ page }) => {
  const state = await fixture(page); await page.goto('/devices?status=offline&page_size=1');
  await expect(page.getByText('Diagnostic phone', { exact: true })).toBeVisible();
  await page.locator('.ant-pagination-item-2').click();
  await expect(page.getByText('Legacy fixture', { exact: true })).toBeVisible();
  expect(state.rows.at(-1)?.searchParams.get('status')).toBe('offline');
  expect(state.stats.at(-1)?.searchParams.has('page')).toBe(false);
  await page.getByRole('combobox', { name: 'System', exact: true }).selectOption('android');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page.getByText('Diagnostic phone', { exact: true })).toBeVisible();
  expect(new URL(page.url()).searchParams.has('page')).toBe(false);
  await expect(page.getByRole('combobox', { name: 'System', exact: true })).toHaveValue('android');
  await page.getByRole('combobox', { name: 'System', exact: true }).selectOption('');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect.poll(() => state.rows.at(-1)?.searchParams.has('system')).toBe(false);
});

test('ordinary users only request their own diagnostics and have no policy controls', async ({ page }) => {
  const state = await fixture(page, { ordinary: true }); await page.goto('/devices?scope=all');
  await expect(page.getByText('Diagnostic phone', { exact: true })).toBeVisible();
  expect(state.rows.at(-1)?.searchParams.get('scope')).toBe('mine'); expect(state.stats.at(-1)?.searchParams.get('scope')).toBe('mine');
  await page.goto('/settings'); await expect(page.getByRole('combobox', { name: 'Runtime override', exact: true })).toHaveCount(0);
});

test('diagnostic distributions are read-only and all filter dropdowns expose selectable options', async ({ page }) => {
  await fixture(page); await page.goto('/devices');
  const diagnostics = page.getByRole('region', { name: 'Device diagnostics', exact: true });
  await expect(diagnostics.locator('.diagnostics-distribution')).toHaveCount(2);
  await expect(diagnostics.locator('.diagnostic-chip')).toHaveCount(4);
  await expect(diagnostics.locator('.diagnostic-chip[tabindex]')).toHaveCount(0);
  await expect(diagnostics.getByRole('button')).toHaveCount(1); // Refresh only.
  await expect(diagnostics.getByRole('group', { name: 'Agent version', exact: true })).toHaveCount(0);
  await expect(diagnostics.locator('.diagnostics-footer')).not.toContainText('Click');
  const before = page.url();
  await diagnostics.getByText('Android', { exact: true }).click();
  await expect(page).toHaveURL(before);
  for (const [name, values] of Object.entries({
    Online: ['', 'online', 'offline'], System: ['', 'windows', 'linux', 'macos', 'android', 'unknown'],
    Architecture: ['', 'x86_64', 'aarch64', 'unknown'], 'Version report status': ['', 'reported', 'unknown'], 'Agent version': ['', '1.2.48'],
  })) {
    const select = page.getByRole('combobox', { name, exact: true });
    expect(await select.locator('option').evaluateAll(options => options.map(option => (option as HTMLOptionElement).value))).toEqual(values);
    await expect(select.locator('option:disabled')).toHaveCount(0);
  }
  const system = page.getByRole('combobox', { name: 'System', exact: true });
  await system.click(); await system.press('Home'); await system.press('ArrowDown'); await system.press('Enter');
  await expect(system).toHaveValue('windows'); await expect(page).toHaveURL(before);
  await page.getByRole('combobox', { name: 'Version report status', exact: true }).selectOption('unknown');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  // A narrow/empty result must not erase the account's available version choices.
  await expect(page.getByRole('combobox', { name: 'Agent version', exact: true }).locator('option')).toHaveText(['All', '1.2.48']);
});

for (const ordinary of [false, true]) test(`diagnostics stay unfiltered across empty results, reload and refresh (${ordinary ? 'personal' : 'admin'})`, async ({ page }) => {
  const state = await fixture(page, { ordinary });
  await page.goto(`/devices?user=${owner}&q=absent&system=android&architecture=aarch64&status=online&version_state=unknown&page=3&page_size=1`);
  const region = page.getByRole('region', { name: 'Device diagnostics', exact: true });
  const verify = async () => {
    await expect(page.locator('.ant-table-tbody .device-link')).toHaveCount(0);
    await expect(region.locator('.diagnostic-metric-total>strong')).toHaveText('2');
    await expect(region.locator('.diagnostic-metric-online>strong')).toHaveText('0');
    await expect(region.locator('.diagnostic-metric-offline>strong')).toHaveText('2');
    await expect(region.locator('.diagnostic-reporting>strong')).toContainText('1 / 2');
    await expect(region.getByRole('group', { name: 'System', exact: true })).toContainText('Android');
    await expect(region.getByRole('group', { name: 'Architecture', exact: true })).toContainText('aarch64');
    expect(state.stats.some(url => url.searchParams.toString() === `scope=${ordinary ? 'mine' : 'all'}`)).toBe(true);
    expect(state.stats.every(url => !['q', 'system', 'architecture', 'status', 'version', 'version_state', 'page', 'page_size'].some(key => url.searchParams.has(key)))).toBe(true);
  };
  await verify();
  await page.getByRole('button', { name: 'Refresh', exact: true }).click(); await verify();
  await page.reload(); await verify();
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await expect(page.locator('.ant-table-tbody .device-link')).toHaveCount(1);
  await expect(region.locator('.diagnostic-metric-total>strong')).toHaveText('2');
});

for (const view of ['settings', 'devices']) for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 390, 768, 1440]) {
  test(`B6 ${view} layout ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await fixture(page, { language, theme }); await page.goto(`/${view}`);
    if (view === 'devices') await expect(page.getByRole('combobox', { name: language === 'en' ? 'Agent version' : language === 'zh-CN' ? '客户端版本' : '用戶端版本', exact: true })).toBeVisible();
    else await expect(page.getByRole('combobox', { name: language === 'en' ? 'Runtime override' : language === 'zh-CN' ? '运行覆盖' : '執行期覆寫', exact: true })).toBeVisible();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
    if (width === 1440) await page.screenshot({ path: screenshotPath(`${view}-${language}-${theme}.png`, '.build/admin-console/20261020-b6'), fullPage: true });
  });
}
