import { isolatedFixture } from './support/isolatedFixture';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { expect, test } from '@playwright/test';
const fixtureConfig = process.env.PAB_ADMIN_FIXTURE_CONFIG;
test.skip(!fixtureConfig, 'Requires the explicitly selected B6 HTTPS fixture');

test('real policy edits survive a process restart and restore the TOML default without losing sign-ins', async ({ page, playwright, baseURL }) => {
  test.setTimeout(60000);
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const config = path.resolve(fixtureConfig!); expect(config).toBe(isolatedFixture().config);
  const root = path.resolve('../..'); const restart = path.join(root, 'scripts/restart_admin_console_test.py');
  const options = { baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } };
  const admin = await playwright.request.newContext(options); const user = await playwright.request.newContext(options);
  const suffix = crypto.randomUUID().slice(0, 12); const adminName = `policy-admin-${suffix}`; const username = `policy-user-${suffix}`; const password = 'isolated live policy password';

  try {
    expect((await admin.post('/api/web/register', { data: { username: adminName, password } })).ok()).toBeTruthy();
    expect((await user.post('/api/web/register', { data: { username, password } })).ok()).toBeTruthy();
    execFileSync(isolatedFixture().executable, ['web-admin', adminName, '--config', config], { stdio: 'pipe', windowsHide: true });
    const initialDiagnostics = await (await admin.get('/api/web/devices/diagnostics?scope=all')).json();
    const initialRevision = (await (await admin.get('/api/web/service')).json()).registration.revision;
    const actor = (await (await admin.get('/api/web/session')).json()).id;
    await page.context().addCookies((await admin.storageState()).cookies); await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto('/settings'); await page.getByRole('combobox', { name: 'Runtime override', exact: true }).selectOption('disabled');
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect.poll(async () => (await (await admin.get('/api/web/service')).json()).registration.override_value).toBe(false);
    for (const endpoint of ['/api/web/config', '/api/account/config']) expect((await (await user.get(endpoint)).json()).registration_enabled).toBe(false);
    for (const endpoint of ['/api/web/register', '/api/account/register']) expect((await user.post(endpoint, { data: { username: `blocked-${suffix}`, password } })).status()).toBe(403);
    expect((await user.get('/api/web/session')).ok()).toBeTruthy();
    execFileSync('python', [restart, '--config', config], { cwd: root, stdio: 'pipe', windowsHide: true });
    await expect.poll(async () => { try { return (await (await admin.get('/api/web/service')).json()).registration.override_value; } catch { return 'restarting'; } }, { timeout: 15000 }).toBe(false);
    expect((await user.get('/api/web/session')).ok()).toBeTruthy();
    await page.reload(); await expect(page.getByRole('button', { name: 'Restore deployment default', exact: true })).toBeEnabled();
    await page.getByRole('button', { name: 'Restore deployment default', exact: true }).click();
    await expect.poll(async () => (await (await user.get('/api/account/config')).json()).registration_enabled).toBe(true);
    const policy = (await (await admin.get('/api/web/service')).json()).registration; expect(policy.override_value).toBeNull(); expect(policy.source).toBe('deployment_default'); expect(policy.revision).toBe(initialRevision + 2);
    const audit = await (await admin.get(`/api/web/audit?action=service.registration_updated&result=succeeded&actor=${actor}`)).json(); expect(audit.total).toBe(2); expect(audit.items[0].before.registration_enabled).toBe(false); expect(audit.items[0].after.registration_source).toBe('deployment_default');
    const diagnostics = await admin.get('/api/web/devices/diagnostics?scope=all'); expect(diagnostics.ok()).toBeTruthy();
    expect((await diagnostics.json()).total).toBe(initialDiagnostics.total);
  } finally {
    await Promise.all([admin.dispose(), user.dispose()]);
    // The owning harness stops the fixture after all browser suites finish.
  }
});
