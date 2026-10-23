import { isolatedFixture } from './support/isolatedFixture';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { expect, test } from '@playwright/test';

// This suite intentionally has no API mocks. An explicitly selected isolated
// fixture is required; never bootstrap an administrator on a normal deployment.
const fixtureConfig = process.env.PAB_ADMIN_FIXTURE_CONFIG;
test.skip(!fixtureConfig, 'Requires scripts/serve_admin_console_test.py and its isolated TOML');
const originalPassword = 'isolated browser fixture original password';
const replacementPassword = 'isolated browser fixture replacement password';

test('real HTTPS password reset revokes old sessions and authenticates the new password', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const options = { baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } };
  const admin = await playwright.request.newContext(options);
  const user = await playwright.request.newContext(options);
  const verifier = await playwright.request.newContext(options);
  const suffix = crypto.randomUUID().slice(0, 12);
  const username = `browser-user-${suffix}`;
  const adminName = `browser-admin-${suffix}`;
  try {
    expect((await admin.post('/api/web/register', { data: { username: adminName, password: originalPassword } })).ok()).toBeTruthy();
    const created = await user.post('/api/web/register', { data: { username, password: originalPassword } });
    expect(created.ok()).toBeTruthy(); const account = await created.json();
    const config = path.resolve(fixtureConfig!);
    expect(config).toBe(isolatedFixture().config);
    execFileSync(isolatedFixture().executable, ['web-admin', adminName, '--config', config], { stdio: 'pipe' });
    expect((await user.get('/api/web/accounts')).status()).toBe(403);
    await page.context().addCookies((await admin.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto(`/accounts/${account.id}`);
    await expect(page.getByRole('heading', { name: username, exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Reset password', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('checkbox')).toBeChecked();
    await dialog.getByLabel('New password', { exact: true }).fill(replacementPassword);
    await dialog.getByLabel('Confirm password', { exact: true }).fill(replacementPassword);
    await dialog.getByRole('button', { name: 'Confirm', exact: true }).click();
    await expect(page.getByTestId('account-operation').first()).toContainText('Server operation committed');
    expect((await user.get('/api/web/session')).status()).toBe(401);
    expect((await verifier.post('/api/web/session', { data: { username, password: originalPassword } })).status()).toBe(401);
    expect((await verifier.post('/api/web/session', { data: { username, password: replacementPassword } })).ok()).toBeTruthy();
    expect((await admin.get('/api/web/session')).ok()).toBeTruthy();
    const audit = await (await admin.get('/api/web/audit')).json();
    expect(JSON.stringify(audit)).not.toContain(replacementPassword);
    expect(JSON.stringify(await page.evaluate(() => Object.fromEntries(Object.entries(localStorage))))).not.toContain(replacementPassword);
  } finally { await Promise.all([admin.dispose(), user.dispose(), verifier.dispose()]); }
});

test('real HTTPS keep-current session action revokes a second login', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const options = { baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } };
  const current = await playwright.request.newContext(options);
  const other = await playwright.request.newContext(options);
  const username = `browser-sessions-${crypto.randomUUID().slice(0, 12)}`;
  try {
    expect((await current.post('/api/web/register', { data: { username, password: originalPassword } })).ok()).toBeTruthy();
    expect((await other.post('/api/web/session', { data: { username, password: originalPassword } })).ok()).toBeTruthy();
    await page.context().addCookies((await current.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto('/account');
    await expect(page.getByText('Current sign-in', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Keep current, revoke other sign-ins', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click();
    await expect(page.getByTestId('account-operation')).toContainText('Server operation committed');
    expect((await other.get('/api/web/session')).status()).toBe(401);
    expect((await current.get('/api/web/session')).ok()).toBeTruthy();
    const sessions = await (await current.get('/api/web/sessions')).json();
    expect(sessions.total).toBe(1); expect(sessions.items[0].current).toBe(true);
    expect(JSON.stringify(sessions)).not.toMatch(/token_hash|access_token|pab_session/);
  } finally { await Promise.all([current.dispose(), other.dispose()]); }
});
