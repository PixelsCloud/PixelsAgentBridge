import { screenshotPath } from './support/isolatedFixture';
import { expect, test, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';

const hour = Math.floor(Date.now() / 3600000) * 3600000;
const from = hour - 7 * 86400000;
const until = hour + 3600000;
const userId = '20545b3a-e6f6-4b91-a122-9072055487ea';
const initial = `/usage?period=7&from=${from}&until=${until}&population=users`;

async function fixture(page: Page, options: { language?: string; theme?: string; previousFail?: boolean; truncated?: boolean; personal?: boolean; manyAccounts?: boolean; dense?: boolean; p2p?: boolean } = {}) {
  const state = { summaries: [] as URL[], rankings: [] as URL[], exports: [] as URL[], accounts: [] as URL[] };
  await page.addInitScript(({ language, theme }) => { localStorage.setItem('pab-web-language', language); localStorage.setItem('pab-web-theme', theme); }, { language: options.language ?? 'en', theme: options.theme ?? 'light' });
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', ''); let data: unknown; let status = 200;
    const bounds = { from: Number(url.searchParams.get('from')), until: Number(url.searchParams.get('until')), interval: url.searchParams.get('interval') ?? 'hour', timezone: 'UTC' };
    if (path === '/config') data = { registration_enabled: true, version: 'fixture' };
    else if (path === '/session') data = { id: '10545b3a-e6f6-4b91-a122-9072055487ea', username: 'Administrator', server_admin: !options.personal };
    else if (path === '/traffic') data = { user_mbps: 10 };
    else if (path === '/accounts') { state.accounts.push(url); const items = [{ id: userId, username: 'Rank user' }].filter(item => item.username.toLowerCase().includes((url.searchParams.get('q') ?? '').toLowerCase())); data = { items, total: options.manyAccounts && !url.searchParams.get('q') ? 101 : items.length }; }
    else if (path === `/accounts/${userId}`) data = { id: userId, username: 'Rank user' };
    else if (path === '/usage') {
      state.summaries.push(url);
      if (options.previousFail && bounds.from < from) { status = 500; data = { code: 'server_error' }; }
      else data = { ...bounds, counters: { uploaded_bytes: 9007199254740992, incomplete: true }, counters_exact: { uploaded_bytes: '9007199254740993', connections: '7' }, incomplete: true, updated_at: hour, points: options.dense ? Array.from({ length: 72 }, (_, i) => ({ hour_unix_ms: bounds.from + i * 3600000, counters_exact: { relay_upload_bytes: String(i * 1048576), relay_download_bytes: String((72 - i) * 1048576) } })) : [{ hour_unix_ms: bounds.from, counters_exact: { uploaded_bytes: '9007199254740993' } }] };
    } else if (path === '/usage/ranking') {
      state.rankings.push(url);
      data = { ...bounds, page: Number(url.searchParams.get('page') ?? 1), page_size: 20, total: 41, items: [{ subject: userId, user_id: userId, username: 'Rank user', counters_exact: { uploaded_bytes: '9007199254740993', relay_download_bytes: '123', connection_ms: '107703661' }, incomplete: true, updated_at: hour }] };
    } else if (path === '/usage/export') {
      state.exports.push(url);
      await route.fulfill({ contentType: 'text/csv; charset=utf-8', headers: { 'content-disposition': 'attachment; filename="pixels-usage.csv"', 'x-usage-truncated': String(!!options.truncated) }, body: '\uFEFF"record_type","from_unix_ms","until_unix_ms","uploaded_bytes","truncated"\r\n"data","' + bounds.from + '","' + bounds.until + '","9007199254740993","false"\r\n"export_status","' + bounds.from + '","' + bounds.until + '","","' + !!options.truncated + '"\r\n' }); return;
    } else throw new Error(`Unexpected usage fixture request: ${path}`);
    if (options.p2p && path === '/usage') data = { ...(data as object), p2p_reported: true, counters_exact: { p2p_upload_bytes: '1536', p2p_download_bytes: '2048' }, points: [{ hour_unix_ms: bounds.from, p2p_reported: true, counters_exact: { p2p_upload_bytes: '1536', p2p_download_bytes: '2048', relay_upload_bytes: '4096', relay_download_bytes: '8192' } }] };
    if (options.p2p && path === '/usage/ranking') data = { ...(data as object), items: [{ subject: userId, user_id: userId, username: 'Rank user', p2p_reported: true, counters_exact: { p2p_upload_bytes: '1536', p2p_download_bytes: '2048' }, updated_at: hour }] };
    await route.fulfill({ status, json: data });
  });
  return state;
}

test('P2P bytes have their own formatted cards, chart, history and ranking without changing Relay limits', async ({ page }) => {
  const state = await fixture(page, { p2p: true });
  await page.goto(initial);
  await expect(page.locator('.usage-metric-card').filter({ hasText: 'P2P upload' })).toContainText('1.5 KiB');
  await expect(page.locator('.usage-metric-card').filter({ hasText: 'P2P download' })).toContainText('2 KiB');
  await expect(page.locator('.usage-chart-legend')).toContainText('P2P upload');
  await expect(page.locator('.usage-chart-legend')).toContainText('Relay upload');
  await page.getByRole('combobox', { name: 'Ranking metric', exact: true }).selectOption('p2p_upload_bytes');
  await expect.poll(() => state.rankings.at(-1)?.searchParams.get('sort')).toBe('p2p_upload_bytes');
  await expect(page.locator('.ant-table-tbody').last()).toContainText('1.5 KiB');
  await page.setViewportSize({ width: 390, height: 1000 });
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(392);
});

test('historical clients without P2P reports do not appear as measured zero', async ({ page }) => {
  await fixture(page); await page.goto(initial);
  await expect(page.locator('.usage-metric-card').filter({ hasText: 'P2P upload' })).toContainText('Not reported');
});

test('successful current data stays quiet when incomplete or previous data fails, and CSV keeps exact bounds', async ({ page }) => {
  const state = await fixture(page, { previousFail: true, truncated: true }); await page.goto(initial);
  await expect(page.getByTitle('9007199254740993 B', { exact: true }).first()).toBeVisible();
  await expect.poll(() => state.summaries.some(url => Number(url.searchParams.get('from')) < from)).toBe(true);
  await expect(page.getByText('Previous period is unavailable', { exact: true })).toHaveCount(0);
  await expect(page.getByText('Some reporting periods have incomplete data.', { exact: true })).toHaveCount(0);
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export CSV', exact: true }).click();
  const file = await download; const csv = await readFile((await file.path())!, 'utf8');
  expect(csv).toContain('9007199254740993'); expect(csv).toContain('export_status');
  expect(state.exports[0].searchParams.get('from')).toBe(String(from)); expect(state.exports[0].searchParams.get('until')).toBe(String(until));
  expect(state.exports[0].searchParams.get('population')).toBe('users');
  await expect(page.getByText('Export reached the 10,000-row limit and is marked as truncated. Choose a smaller range to export all rows.', { exact: true })).toBeVisible();
});

test('ranking pagination sorting and account links survive reload with the same window', async ({ page }) => {
  const state = await fixture(page); await page.goto(initial);
  const ranking = page.getByRole('heading', { name: 'Usage ranking', exact: true });
  await expect(page.getByRole('button', { name: 'Rank user', exact: true })).toBeVisible();
  await page.getByRole('combobox', { name: 'Ranking metric', exact: true }).selectOption('uploaded_bytes');
  await page.getByRole('combobox', { name: 'Sort direction', exact: true }).selectOption('asc');
  await page.locator('.ant-pagination-item-2').last().click();
  await expect.poll(() => state.rankings.at(-1)?.searchParams.get('page')).toBe('2');
  await page.reload(); await expect.poll(() => state.rankings.at(-1)?.searchParams.get('order')).toBe('asc');
  await page.getByRole('button', { name: 'Rank user', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`user=${userId}`));
  const url = new URL(page.url()); expect(url.searchParams.get('from')).toBe(String(from)); expect(url.searchParams.get('until')).toBe(String(until)); expect(url.searchParams.get('population')).toBe('users');
});

test('long ranges select daily data and expired hourly ranges show unavailable without fake zeros', async ({ page }) => {
  const state = await fixture(page); await page.goto('/usage?period=180');
  await expect.poll(() => state.summaries.at(-1)?.searchParams.get('interval')).toBe('day');
  await page.goto('/usage?period=180&interval=hour');
  await expect(page.getByText('This range is outside retention. Select daily resolution or a smaller range.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Export CSV', exact: true })).toBeDisabled();
  await expect(page.getByTitle('9007199254740993 B', { exact: true })).toHaveCount(0);
});

test('personal usage removes scope overrides and has no admin ranking', async ({ page }) => {
  const state = await fixture(page, { personal: true }); await page.goto(initial + `&user=${userId}`);
  await expect.poll(() => state.summaries.length).toBeGreaterThan(0);
  expect(state.summaries[0].searchParams.get('scope')).toBe('mine'); expect(state.summaries[0].searchParams.has('user')).toBe(false); expect(state.rankings).toHaveLength(0);
});

test('all usage dropdowns accept pointer and keyboard choices and format history and ranking bytes', async ({ page }) => {
  const state = await fixture(page); await page.goto('/usage');
  for (const [name, expected] of [['Usage period', 'month'], ['Time resolution', 'hour'], ['Usage attribution', 'users'], ['Ranking metric', 'relay_download_bytes'], ['Sort direction', 'asc']]) {
    const select = page.getByRole('combobox', { name, exact: true });
    await expect(select.locator('option')).not.toHaveCount(1);
    await select.click(); await select.press('Home'); await select.press('ArrowDown'); await select.press('Enter');
    await expect(select).toHaveValue(expected);
  }
  await expect(page.getByRole('combobox', { name: 'Username', exact: true })).toHaveCount(0);
  expect(state.accounts).toHaveLength(0);
  expect(state.rankings.at(-1)?.searchParams.has('user')).toBe(false);
  expect(state.rankings.at(-1)?.searchParams.get('order')).toBe('asc');
  await page.getByRole('combobox', { name: 'Ranking metric', exact: true }).selectOption('uploaded_bytes');
  await expect(page.locator('.ant-table-row').filter({ has: page.getByRole('button', { name: 'Rank user', exact: true }) })).toContainText('8 PiB');
  await expect(page.locator('.usage-metric-card').filter({ hasText: 'Completed upload bytes' })).toContainText('8 PiB');
  await expect(page.locator('.ant-table-cell').filter({ has: page.getByTitle('9007199254740993 B', { exact: true }) })).not.toHaveCount(0);
  await expect(page.getByText('9007199254740993 B', { exact: true })).toHaveCount(0);
  await expect(page.getByText(/CSV preserves decimal counters/)).toHaveCount(0);
  await expect(page.getByText(/Hourly history: 90 days/)).toHaveCount(0);
  await expect(page.getByText(/Relay bytes and client connection/)).toHaveCount(0);
  await page.getByRole('combobox', { name: 'Ranking metric', exact: true }).selectOption('connection_ms');
  await expect(page.locator('.ant-table-row').filter({ has: page.getByRole('button', { name: 'Rank user', exact: true }) })).toContainText('1 day 5 hr 55 min 3 sec');
  await expect(page.getByRole('columnheader', { name: 'Completeness', exact: true })).toHaveCount(0);
  await page.getByRole('combobox', { name: 'Usage attribution', exact: true }).selectOption('guests');
  await expect(page.getByRole('combobox', { name: 'Username', exact: true })).toHaveCount(0);
  await expect.poll(() => state.summaries.at(-1)?.searchParams.get('population')).toBe('guests');
  expect(state.summaries.at(-1)?.searchParams.has('user')).toBe(false);
});

test('usage never enumerates accounts for a username picker', async ({ page }) => {
  const state = await fixture(page, { manyAccounts: true }); await page.goto('/usage');
  await expect(page.getByRole('button', { name: 'Rank user', exact: true })).toBeVisible();
  await expect(page.getByRole('textbox', { name: 'Username', exact: true })).toHaveCount(0);
  await expect(page.getByRole('combobox', { name: 'Username', exact: true })).toHaveCount(0);
  expect(state.accounts).toHaveLength(0);
});

test('72 real-sized chart buckets fit a narrow screen without leaking outside the plot', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 1000 }); await fixture(page, { dense: true }); await page.goto(initial);
  await expect(page.locator('.usage-chart-bucket')).toHaveCount(72);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(392);
  const plot = await page.locator('.usage-chart-plot').boundingBox();
  const finalBar = await page.locator('.usage-chart-bar').last().boundingBox();
  expect(finalBar!.x + finalBar!.width).toBeLessThanOrEqual(plot!.x + plot!.width + 1);
});

for (const [zone, expectedStart] of [['America/New_York', '2026-10-01T04:00:00.000Z'], ['Asia/Kathmandu', '2026-09-30T18:00:00.000Z'], ['Australia/Adelaide', '2026-09-30T14:00:00.000Z']]) {
  test.describe(zone, () => {
    test.use({ timezoneId: zone });
    test('custom local range is visibly rounded to the UTC data resolution and reused for export', async ({ page }) => {
      const state = await fixture(page); await page.goto(initial);
      await page.getByRole('combobox', { name: 'Usage period', exact: true }).selectOption('custom');
      await page.getByLabel('Range start (inclusive)', { exact: true }).fill('2026-10-01T00:00');
      await page.getByLabel('Range end (exclusive)', { exact: true }).fill('2026-10-02T00:00');
      await page.getByRole('button', { name: 'Apply range', exact: true }).click();
      await expect(page.getByTestId('usage-window')).toHaveAttribute('title', new RegExp(expectedStart));
      await page.reload(); await expect(page.getByTestId('usage-window')).toHaveAttribute('title', new RegExp(expectedStart));
      const download = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export CSV', exact: true }).click(); await download;
      expect(state.exports.at(-1)?.searchParams.get('from')).toBe(String(Date.parse(expectedStart)));
    });
  });
}

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 390, 768, 1440]) {
  test(`usage layout ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await fixture(page, { language, theme }); await page.goto(initial);
    await expect(page.getByTitle('9007199254740993 B', { exact: true }).first()).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
    if (width === 1440) await page.screenshot({ path: screenshotPath(`usage-${language}-${theme}.png`, '.build/admin-console/20261020-b4'), fullPage: true });
  });
}
