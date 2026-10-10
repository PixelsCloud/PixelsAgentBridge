import { expect, test, type Page } from '@playwright/test';

async function fixture(page: Page, admin = true) {
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
    else if (path === '/devices') {
      requests.push(url);
      data = { items: [], total: 0, page: 1, page_size: 20 };
    } else if (path === '/accounts') data = { items: [{ ...viewer, status: 'active', revision: 1, relay_limit_mbps: limit }], total: 1, page: 1, page_size: 20 };
    else if (path === '/accounts/test-user/traffic') {
      const body = route.request().postDataJSON(); writes.push(body); limit = body.mbps; data = {};
    } else throw new Error(`Unexpected fixture request: ${path}`);
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
  await page.getByRole('combobox', { name: 'Online', exact: true }).click();
  await page.locator('.ant-select-item-option-content').getByText('Offline', { exact: true }).click();
  await expect(page).toHaveURL('/devices?status=offline');
  await page.reload();
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('status')).toBe('offline');
  await page.goto('/online?q=fixture');
  await expect(page).toHaveURL('/devices?q=fixture&status=online');
  await page.getByRole('combobox', { name: 'Online', exact: true }).click();
  await page.locator('.ant-select-item-option-content').getByText('All', { exact: true }).click();
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
  await dialog.getByRole('combobox').click();
  await expect(page.locator('.ant-select-item-option-content')).toHaveText([5,10,20,30,40,50,60,70,80,90,100].map(v => `${v} Mbps`));
  await page.locator('.ant-select-item-option-content').getByText('30 Mbps', { exact: true }).click();
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('30 Mbps', { exact: true })).toBeVisible();
  await open();
  await expect(dialog.locator('.ant-select')).toContainText('30 Mbps');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(state.writes).toEqual([{ mbps: 30 }]);
});
