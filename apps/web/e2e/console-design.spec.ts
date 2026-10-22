import { mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { test, expect } from '@playwright/test';
import { designAccountId, designDeviceId, designFixture } from './support/designFixture';

const images = resolve(import.meta.dirname, '../../../design/admin/2026-10-21/images');
const views = [
  ['overview', '/'], ['devices', '/devices'], ['device-detail', `/devices/${designDeviceId}`],
  ['accounts', '/accounts'], ['account-detail', `/accounts/${designAccountId}`], ['usage', '/usage'],
  ['relay', '/relay'], ['audit', '/audit'], ['settings', '/settings'], ['account', '/account'], ['saved-devices', '/saved-devices'],
] as const;

for (const theme of ['light', 'dark']) for (const [name, path] of views) {
  test(`design preview ${name} ${theme}`, async ({ page }) => {
    const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
    await page.setViewportSize({ width: 1440, height: 1040 }); await designFixture(page, { theme });
    await page.goto(path); await expect(page.locator('.console-content .ant-spin-spinning')).toHaveCount(0);
    await expect(page.locator('.console-content')).toBeVisible(); await expect(page.getByRole('status')).toContainText('实时同步');
    await expect(page.locator('.console-workspace, .workspace-shortcut, .overview-guide')).toHaveCount(0);
    if (name === 'overview') {
      await expect(page.locator('.page-heading-actions')).toHaveCount(0);
      await expect(page.locator('.admin-overview .metric')).toHaveCount(4);
      await expect(page.locator('.quick-access-list, .connectivity-chart')).toHaveCount(0);
      expect((await page.locator('.admin-overview .metrics').boundingBox())!.height).toBeLessThanOrEqual(160);
    }
    await expect(page.locator('.console-content .ant-skeleton')).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(1442);
    mkdirSync(images, { recursive: true }); await page.screenshot({ path: resolve(images, `${name}-${theme}.png`), fullPage: true, animations: 'disabled' });
    expect(errors).toEqual([]);
  });
}

for (const width of [360, 768]) for (const theme of ['light', 'dark']) {
  test(`design adaptive navigation ${width} ${theme}`, async ({ page }) => {
    const state = await designFixture(page, { theme, language: 'en' }); await page.setViewportSize({ width, height: 1000 }); await page.goto('/');
    await expect(page.getByRole('heading', { name: 'Overview.', exact: true })).toBeVisible();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
    await page.screenshot({ path: resolve(images, `overview-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
    if (width < 768) await page.getByRole('button', { name: 'Navigation menu', exact: true }).click();
    await page.getByRole('menuitem', { name: 'Accounts', exact: true }).click(); await expect(page).toHaveURL('/accounts');
    if (width < 768) await page.getByRole('button', { name: 'Navigation menu', exact: true }).click();
    await expect(page.locator('.console-workspace, .workspace-shortcut')).toHaveCount(0);
    await page.getByRole('button', { name: 'Open personal center', exact: true }).click(); await expect(page).toHaveURL('/');
    await expect(page.locator('.quick-access-list').getByRole('button', { name: /^My devices/ })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: 'Accounts', exact: true })).toHaveCount(0);
    if (width < 768) await page.getByRole('button', { name: 'Navigation menu', exact: true }).click();
    await page.getByRole('button', { name: 'Back to service management', exact: true }).click();
    if (width < 768) await page.getByRole('button', { name: 'Navigation menu', exact: true }).click();
    await expect(page.getByRole('menuitem', { name: 'Accounts', exact: true })).toBeVisible();
    expect(state.reads.filter(path => path === '/overview').length).toBeGreaterThan(0); expect(state.writes).toEqual([]);
  });
}

for (const empty of [true, false]) test(`overview ${empty ? 'empty' : 'unavailable'} preserves absent counts`, async ({ page }) => {
  await designFixture(page, { language: 'en', empty, unavailable: !empty }); await page.goto('/');
  await expect(page.locator('.connectivity-chart')).toHaveCount(0);
  await expect(page.locator('.metric-value').first()).toHaveText(empty ? '0' : '—');
});

for (const width of [360, 1440]) for (const theme of ['light', 'dark']) test(`sign in design ${width} ${theme}`, async ({ page }) => {
  await designFixture(page, { theme, signedOut: true }); await page.setViewportSize({ width, height: 1000 }); await page.goto('/');
  await expect(page.getByRole('heading', { name: '登录', exact: true })).toBeVisible();
  await expect(page.locator('.auth-eyebrow, .auth-features, .auth-story > p')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
  await page.screenshot({ path: resolve(images, `login-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
});

test('traffic chart preserves exact large bytes and leaves unreported series unknown', async ({ page }) => {
  await designFixture(page, { language: 'en', ordinary: true });
  await page.route('**/api/web/usage?**', async route => {
    const url = new URL(route.request().url()); const from = Number(url.searchParams.get('from'));
    await route.fulfill({ json: { from, until: Number(url.searchParams.get('until')), interval: 'hour', timezone: 'UTC', updated_at: Date.now(), counters: {}, counters_exact: {}, incomplete: false,
      points: [{ hour_unix_ms: from, counters_exact: { relay_upload_bytes: '9007199254740993' } }] } });
  });
  await page.goto('/usage');
  await expect(page.locator('.usage-chart-bar.usage-series-0')).toHaveAttribute('title', /8 PiB \(9007199254740993 B\)$/);
  await expect(page.locator('.usage-chart-bar.usage-series-0')).toHaveCSS('height', '172px');
  await expect(page.locator('.usage-chart-bar.usage-series-1')).toHaveAttribute('title', /Unknown$/);
  await expect(page.locator('.usage-chart-bar.usage-series-1')).toHaveCSS('height', '0px');
});

for (const register of [false, true]) test(`redesigned ${register ? 'registration' : 'sign in'} submits the original API contract`, async ({ page }) => {
  await designFixture(page, { language: 'en', signedOut: true });
  const requests: unknown[] = [];
  await page.route(`**/api/web/${register ? 'register' : 'session'}`, async route => {
    if (route.request().method() === 'GET') { await route.fallback(); return; }
    requests.push(route.request().postDataJSON());
    await route.fulfill({ json: { id: designAccountId, username: 'preview-user', server_admin: false, personal_tenant_id: 'preview-tenant' } });
  });
  await page.goto('/');
  if (register) await page.getByRole('button', { name: 'Create an account', exact: true }).click();
  await page.getByLabel('Username', { exact: true }).fill('preview-user');
  await page.getByLabel('Password', { exact: true }).fill('preview-only-password');
  if (register) {
    await page.getByLabel('Confirm password', { exact: true }).fill('does-not-match');
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await expect(page.getByText('Passwords do not match.', { exact: true })).toBeVisible(); expect(requests).toEqual([]);
    await page.getByLabel('Confirm password', { exact: true }).fill('preview-only-password');
  }
  await page.getByRole('button', { name: register ? 'Create account' : 'Sign in', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Overview', exact: true })).toBeVisible();
  expect(requests).toEqual([{ username: 'preview-user', password: 'preview-only-password' }]);
  await expect(page.getByRole('menuitem', { name: 'Accounts', exact: true })).toHaveCount(0);
});
