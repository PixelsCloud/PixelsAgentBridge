import { screenshotPath } from './support/isolatedFixture';
import { expect, test, type Page } from '@playwright/test';
import { readFile } from 'node:fs/promises';

const actor = '20545b3a-e6f6-4b91-a122-9072055487ea';
const target = '30545b3a-e6f6-4b91-a122-9072055487ea';
async function fixture(page: Page, options: { language?: string; theme?: string; truncated?: boolean; exportFailed?: boolean; ordinary?: boolean } = {}) {
  const state = { lists: [] as URL[], exports: [] as URL[] };
  await page.addInitScript(({ language, theme }) => { localStorage.setItem('pab-web-language', language); localStorage.setItem('pab-web-theme', theme); }, { language: options.language ?? 'en', theme: options.theme ?? 'light' });
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', ''); let data: unknown;
    if (path === '/config') data = { registration_enabled: true, version: 'fixture' };
    else if (path === '/session') data = { id: actor, username: 'Current administrator name', server_admin: !options.ordinary };
    else if (path === '/traffic') data = { user_mbps: 10 };
    else if (path === '/overview') data = {};
    else if (path === '/audit') {
      state.lists.push(url); const second = url.searchParams.get('cursor') === 'second';
      data = { total: 41, page_size: 20, snapshot_frozen: true, snapshot_cursor: 'frozen', next_cursor: second ? null : 'second', previous_cursor: second ? 'frozen' : null, items: [{
        id: second ? 'web:20' : 'web:41', actor: 'Captured administrator name', actor_id: actor, action: 'account.password_reset', result: 'succeeded', created_at: Date.parse('2026-10-20T01:00:00Z'),
        resource_kind: 'account', resource_key: target, resource_id: target, resource_name: second ? 'Second page target' : 'Captured target name',
        request_id: '40545b3a-e6f6-4b91-a122-9072055487ea', operation_id: '50545b3a-e6f6-4b91-a122-9072055487ea',
        change_state: 'recorded', before: { revision: 1 }, after: { revision: 2, password_changed: true }, details: { sessions_revoked: 3 },
      }, { id: 'web:1', actor: null, actor_id: null, action: 'admin.bootstrap', result: 'historical_unknown', created_at: Date.parse('2026-10-19T01:00:00Z'), resource_name: null, resource_kind: null, resource_key: null, resource_id: null, request_id: null, operation_id: null, change_state: 'historical_unknown', before: null, after: null, details: null }] };
    } else if (path === '/audit/export') {
      state.exports.push(url);
      if (options.exportFailed) { await route.fulfill({ status: 500, json: { code: 'server_error' } }); return; }
      await route.fulfill({ contentType: 'text/csv; charset=utf-8', headers: { 'x-audit-truncated': String(!!options.truncated) }, body: '\uFEFF"record_type","actor_name","after","rows","truncated"\r\n"data","Captured administrator name","changed",,\r\n"export_status",,,"1","' + !!options.truncated + '"\r\n' }); return;
    } else throw new Error(`Unexpected audit fixture request: ${path}`);
    await route.fulfill({ json: data });
  });
  return state;
}

test('compound filters survive reload and share the exact frozen snapshot with CSV', async ({ page }) => {
  const state = await fixture(page, { truncated: true }); await page.goto('/audit');
  await expect(page.getByText('Captured target name', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Search actions or name snapshots', { exact: true })).toHaveCount(0);
  await expect(page.getByLabel('Actor account ID', { exact: true })).toHaveCount(0);
  await expect(page.getByText(/Time filters use/)).toHaveCount(0);
  await expect(page.getByText(/Denied and failed operations/)).toHaveCount(0);
  await page.getByLabel('Resource or node ID', { exact: true }).fill(target);
  await page.getByLabel('Start time (inclusive)', { exact: true }).fill('2026-10-01T00:00');
  await page.getByLabel('End time (exclusive)', { exact: true }).fill('2026-10-21T00:00');
  await page.getByRole('combobox', { name: 'Outcome', exact: true }).selectOption('succeeded');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect.poll(() => state.lists.at(-1)?.searchParams.get('resource')).toBe(target);
  await expect(page).toHaveURL(/cursor=frozen/); await page.reload();
  await expect(page.getByLabel('Resource or node ID', { exact: true })).toHaveValue(target);
  const download = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export audit CSV', exact: true }).click();
  const csv = await readFile((await (await download).path())!, 'utf8'); expect(csv).toContain('export_status');
  const exported = state.exports.at(-1)!; expect(exported.searchParams.get('cursor')).toBe('frozen'); expect(exported.searchParams.has('actor')).toBe(false); expect(exported.searchParams.get('resource')).toBe(target); expect(exported.searchParams.get('from')).toBeTruthy(); expect(exported.searchParams.get('result')).toBe('succeeded');
  await expect(page.getByText('Audit export reached the 10,000-row limit and is marked as truncated. Narrow the filters.', { exact: true })).toBeVisible();
});

test('keyset navigation stays frozen and filter changes or refresh discard its cursor', async ({ page }) => {
  const state = await fixture(page); await page.goto('/audit?q=original'); await expect(page).toHaveURL(/cursor=frozen/);
  await page.getByRole('button', { name: 'Next page', exact: true }).click(); await expect(page.getByText('Second page target', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Previous page', exact: true }).click(); await expect(page.getByText('Captured target name', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect.poll(() => state.lists.filter(u => !u.searchParams.has('cursor')).length).toBeGreaterThanOrEqual(2);
  await page.getByRole('button', { name: 'Clear filters', exact: true }).click();
  await expect(page.getByLabel('Resource or node ID', { exact: true })).toHaveValue('');
  await expect.poll(() => state.lists.at(-1)?.searchParams.has('q')).toBe(false);
});

test('changes expose correlation IDs and password marker; historical fields stay explicitly unknown', async ({ page }) => {
  await fixture(page); await page.goto('/audit');
  await page.getByRole('button', { name: 'Expand row', exact: true }).first().click();
  await expect(page.getByText('40545b3a-e6f6-4b91-a122-9072055487ea', { exact: true })).toBeVisible();
  await expect(page.getByText('Changed', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Expand row', exact: true }).first().click();
  await expect(page.getByText('Historical name snapshots or changes were not collected. Current values are not substituted.', { exact: true })).toBeVisible();
  expect(await page.locator('main').innerText()).not.toMatch(/password_hash|password_length|access_token/);
});

test('invalid ranges never issue a new query and CSV failures preserve the current view', async ({ page }) => {
  const state = await fixture(page, { exportFailed: true }); await page.goto('/audit'); await expect(page).toHaveURL(/cursor=frozen/);
  await expect(page.getByText('Captured target name', { exact: true })).toBeVisible();
  // Freezing the URL legitimately starts a second, cursor-bound read. Wait for
  // that read to finish before checking that invalid input issues no new query.
  await expect.poll(() => state.lists.at(-1)?.searchParams.get('cursor')).toBe('frozen');
  await expect(page.getByRole('button', { name: 'Export audit CSV', exact: true })).toBeEnabled();
  const count = state.lists.length;
  await page.getByLabel('Start time (inclusive)', { exact: true }).fill('2026-10-21T00:00');
  await page.getByLabel('End time (exclusive)', { exact: true }).fill('2026-10-20T00:00');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page.locator('.ant-form-item-explain-error')).toBeVisible(); expect(state.lists.length).toBe(count);
  await page.getByRole('button', { name: 'Export audit CSV', exact: true }).click();
  await expect(page.getByText('Audit export failed', { exact: true })).toBeVisible(); await expect(page.getByText('Captured target name', { exact: true })).toBeVisible();
});

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 390, 768, 1440]) {
  test(`audit layout ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await fixture(page, { language, theme }); await page.goto('/audit');
    await expect(page.getByText('Captured target name', { exact: true })).toBeVisible();
    await page.locator('.ant-table-row-expand-icon').first().click();
    await expect(page.getByText('40545b3a-e6f6-4b91-a122-9072055487ea', { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
    if (width === 1440) await page.screenshot({ path: screenshotPath(`audit-${language}-${theme}.png`, '.build/admin-console/20261020-b5'), fullPage: true });
  });
}
