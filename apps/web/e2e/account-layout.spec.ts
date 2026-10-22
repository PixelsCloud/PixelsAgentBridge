import { expect, test, type Page } from '@playwright/test';
import { designFixture } from './support/designFixture';

async function accountFixture(page: Page, options: { language?: string; theme?: string; githubOnly?: boolean; githubOff?: boolean } = {}) {
  await designFixture(page, options);
  const now = Date.now();
  const sessions = Array.from({ length: 17 }, (_, index) => ({ id: `52000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`, channel: index % 2 ? 'native' : 'browser', current: index === 0, created_at: now - (index + 1) * 86400000, last_active_at: now - index * 300000 }));
  const reads: URL[] = []; const writes: { path: string; body: unknown }[] = [];
  await page.route('**/api/web/github', route => route.fulfill({ json: { enabled: !options.githubOff, login: options.githubOff ? null : 'pixels-team', password_enabled: !options.githubOnly } }));
  await page.route('**/api/web/sessions?*', route => {
    const url = new URL(route.request().url()); reads.push(url);
    const size = Number(url.searchParams.get('page_size') ?? 20); const current = Number(url.searchParams.get('page') ?? 1);
    return route.fulfill({ json: { items: sessions.slice((current - 1) * size, current * size), total: sessions.length, page: current, page_size: size } });
  });
  await page.route('**/api/web/password', route => {
    writes.push({ path: new URL(route.request().url()).pathname, body: route.request().postDataJSON() });
    return route.fulfill({ status: 204 });
  });
  return { reads, writes, sessions };
}

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 1440]) {
  test(`my account remains readable and compact ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await accountFixture(page, { language, theme }); await page.goto('/account');
    await expect(page.locator('.account-password input')).toHaveCount(3);
    await expect(page.locator('.account-sessions .ant-table-row')).toHaveCount(5);
    await expect(page.locator('.account-github')).toContainText('pixels-team');
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 1);
    if (width === 1440) {
      expect(await page.evaluate(() => document.documentElement.scrollHeight)).toBeLessThanOrEqual(900);
      const password = (await page.locator('.account-password').boundingBox())!;
      const sessions = (await page.locator('.account-sessions').boundingBox())!;
      expect(Math.abs(password.y - sessions.y)).toBeLessThan(2);
      expect(sessions.x).toBeGreaterThan(password.x + password.width);
    }
    const small = await page.locator('.account-label, .account-sessions th, .account-sessions td, .account-session-id, .account-password label').evaluateAll(nodes => nodes.filter(node => Number.parseFloat(getComputedStyle(node).fontSize) < 13).map(node => node.className));
    expect(small).toEqual([]);
  });
}

test('my account pages sessions on the server and retains complete session identifiers', async ({ page }) => {
  const state = await accountFixture(page, { language: 'en' }); await page.goto('/account');
  await expect(page.locator('.account-sessions .ant-table-row')).toHaveCount(5);
  expect(state.reads[0].searchParams.get('page_size')).toBe('5');
  await expect(page.locator('.account-session-id').first()).toHaveAttribute('title', state.sessions[0].id);
  await page.locator('.account-sessions .ant-pagination-item-2').click();
  await expect.poll(() => state.reads.at(-1)?.searchParams.get('page')).toBe('2');
  await expect(page.locator('.account-session-id').first()).toHaveAttribute('title', state.sessions[5].id);
});

test('compact password form preserves confirmation validation and the existing API contract', async ({ page }) => {
  const state = await accountFixture(page, { language: 'en' }); await page.goto('/account');
  await page.getByLabel('Current password', { exact: true }).fill('fixture existing password');
  await page.getByLabel('New password', { exact: true }).fill('fixture replacement password');
  await page.getByLabel('Confirm password', { exact: true }).fill('different password');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Passwords do not match.', { exact: true })).toBeVisible();
  expect(state.writes).toEqual([]);
  await page.getByLabel('Confirm password', { exact: true }).fill('fixture replacement password');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.getByText('Password updated.', { exact: true })).toBeVisible();
  expect(state.writes).toEqual([{ path: '/api/web/password', body: { current_password: 'fixture existing password', new_password: 'fixture replacement password' } }]);
});

test('GitHub-only accounts retain their only sign-in method and use the full session column', async ({ page }) => {
  await accountFixture(page, { language: 'en', githubOnly: true }); await page.goto('/account');
  await expect(page.locator('.account-sessions .ant-table-row')).toHaveCount(5);
  await expect(page.locator('.account-password')).toHaveCount(0);
  await expect(page.locator('.account-sessions-only')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Unlink', exact: true })).toBeDisabled();
});
