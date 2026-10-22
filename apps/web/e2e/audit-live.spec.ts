import { isolatedFixture } from './support/isolatedFixture';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';

const fixtureConfig = process.env.PAB_ADMIN_FIXTURE_CONFIG;
test.skip(!fixtureConfig, 'Requires the isolated HTTPS fixture TOML');

test('real HTTPS audit freezes a filtered view, exports it, and refreshes later commits', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const config = path.resolve(fixtureConfig!);
  expect(config).toBe(isolatedFixture().config);
  const options = { baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } };
  const admin = await playwright.request.newContext(options); const user = await playwright.request.newContext(options);
  const suffix = crypto.randomUUID().slice(0, 12); const adminName = `audit-admin-${suffix}`; const username = `audit-target-${suffix}`;
  const password = 'isolated live audit password';
  try {
    expect((await admin.post('/api/web/register', { data: { username: adminName, password } })).ok()).toBeTruthy();
    const created = await user.post('/api/web/register', { data: { username, password } }); expect(created.ok()).toBeTruthy(); const account = await created.json();
    execFileSync(path.join(path.dirname(config), 'pab-server.exe'), ['web-admin', adminName, '--config', config], { stdio: 'pipe' });
    const route = `/api/web/accounts/${account.id}/traffic`;
    const first = await admin.put(route, { data: { mbps: 10 } }); expect(first.ok()).toBeTruthy(); const requestId = first.headers()['x-pixels-request-id']; expect(requestId).toMatch(/^[0-9a-f-]{36}$/);
    await page.context().addCookies((await admin.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto(`/audit?action=account.relay_limit_updated&resource_kind=account&resource=${account.id}`);
    await expect(page).toHaveURL(/cursor=/); await expect(page.getByText(username, { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Expand row', exact: true }).click(); await expect(page.getByText(requestId, { exact: true })).toBeVisible();
    const query = new URL(page.url()).search;
    const frozen = await (await admin.get('/api/web/audit' + query)).json(); expect(frozen.total).toBe(1); expect(frozen.items[0].after).toEqual({ mbps: 10 });
    expect((await admin.put(route, { data: { mbps: 20 } })).ok()).toBeTruthy();
    const original = await (await admin.get('/api/web/audit' + query)).json(); expect(original.total).toBe(1); expect(original.items).toEqual(frozen.items);
    const download = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export audit CSV', exact: true }).click();
    const csv = await readFile((await (await download).path())!, 'utf8'); expect(csv).toContain(requestId); expect(csv).not.toContain(password); expect(csv.split('\n').filter(line => line.startsWith('"data"'))).toHaveLength(1); expect(csv).toContain('export_status');
    await page.getByRole('button', { name: 'Refresh', exact: true }).click();
    await expect(page.getByText(username, { exact: true })).toHaveCount(2);
    const refreshed = await (await admin.get('/api/web/audit' + new URL(page.url()).search)).json(); expect(refreshed.total).toBe(2); expect(refreshed.items[0].before).toEqual({ mbps: 10 }); expect(refreshed.items[0].after).toEqual({ mbps: 20 });
    for (let n = 0; n < 4; n++) expect((await user.put(route, { data: { mbps: 50 } })).status()).toBe(403);
    const denied = await (await admin.get(`/api/web/audit?action=account.relay_limit_updated&result=denied&actor=${account.id}`)).json(); expect(denied.total).toBe(1); expect(denied.items[0].details.error_code).toBe('forbidden');
    expect((await user.get('/api/web/audit/export')).status()).toBe(403);
  } finally { await Promise.all([admin.dispose(), user.dispose()]); }
});
