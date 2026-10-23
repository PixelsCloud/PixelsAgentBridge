import { expect, test, type Page } from '@playwright/test';
import { designFixture } from './support/designFixture';

async function fixture(page: Page, admin = true, total = 0) {
  await designFixture(page, { language: 'en', ordinary: !admin });
  let limit = 10;
  const writes: unknown[] = [];
  const requests: URL[] = [];
  await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url());
    const path = url.pathname.replace('/api/web', '');
    const viewer = { id: 'test-user', username: 'Fixture user', server_admin: admin };
    let data: unknown;
    if (path === '/config') data = { registration_enabled: true, version: 'test' };
    else if (path === '/session') data = viewer;
    else if (path === '/overview') data = { total: 3, online: 2, offline: 1 };
    else if (path === '/devices/diagnostics') data = { total: 0, reported: 0, not_reported: 0, systems: [], architectures: [], versions: [], versions_truncated: false };
    else if (path === '/devices') {
      requests.push(url);
      data = { items: [], total, page: Number(url.searchParams.get('page') ?? 1), page_size: Number(url.searchParams.get('page_size') ?? 20) };
    } else if (path === '/accounts') data = { items: [{ ...viewer, status: 'active', revision: 1, relay_limit_mbps: limit }], total: 1, page: 1, page_size: 20 };
    else if (path === '/accounts/test-user/traffic') {
      const body = route.request().postDataJSON(); writes.push(body); limit = body.mbps; data = {};
    } else return route.fallback();
    await route.fulfill({ json: data });
  });
  return { writes, requests };
}

for (const admin of [true, false]) test(`device statistics and filters share one page (${admin ? 'admin' : 'personal'})`, async ({ page }) => {
  const state = await fixture(page, admin);
  await page.goto('/');
  await expect(page.getByRole('menuitem', { name: 'Online devices', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Online', exact: true }).click();
  await expect(page).toHaveURL('/devices?status=online');
  await expect(page.getByRole('heading', { name: admin ? 'Device list' : 'My devices', exact: true })).toBeVisible();
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('status')).toBe('online');
  expect(state.requests.at(-1)?.searchParams.get('scope')).toBe(admin ? 'all' : 'mine');
  await page.getByRole('combobox', { name: 'Online', exact: true }).selectOption('offline');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page).toHaveURL('/devices?status=offline');
  await page.reload();
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('status')).toBe('offline');
  await page.goto('/online?q=fixture');
  await expect(page).toHaveURL('/devices?q=fixture&status=online');
  await page.getByRole('combobox', { name: 'Online', exact: true }).selectOption('');
  await page.getByRole('button', { name: 'Apply filters', exact: true }).click();
  await expect(page).toHaveURL('/devices?q=fixture');
});

test('bandwidth uses explicit presets with a ten Mbps default', async ({ page }) => {
  const state = await fixture(page);
  await page.goto('/accounts');
  const row = page.getByRole('row').filter({ hasText: 'Fixture user' });
  await expect(row.getByText('10 Mbps', { exact: true })).toBeVisible();
  const open = () => row.getByRole('button', { name: 'Bandwidth limits', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await open();
  await expect(dialog.getByRole('switch')).toHaveCount(0);
  await expect(dialog.getByRole('spinbutton')).toHaveCount(0);
  await expect(dialog.getByRole('combobox').locator('option')).toHaveText([5,10,20,30,40,50,60,70,80,90,100].map(v => `${v} Mbps`));
  await dialog.getByRole('combobox').selectOption('30');
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('30 Mbps', { exact: true })).toBeVisible();
  await open();
  await expect(dialog.getByRole('combobox')).toHaveValue('30');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(state.writes).toEqual([{ mbps: 30 }]);
});

test('native page size selection applies numeric size and preserves device filters', async ({ page }) => {
  const state = await fixture(page, true, 41); await page.goto('/devices?status=offline&q=fixture&page=2');
  const size = page.getByRole('combobox', { name: 'Rows per page', exact: true });
  await expect(size).toHaveValue('20');
  await size.selectOption('50');
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('page_size')).toBe('50');
  expect(state.requests.at(-1)?.searchParams.get('status')).toBe('offline');
  expect(state.requests.at(-1)?.searchParams.get('q')).toBe('fixture');
  await expect(size).toHaveValue('50');
  await expect(page.locator('.ant-select')).toHaveCount(0);
});
