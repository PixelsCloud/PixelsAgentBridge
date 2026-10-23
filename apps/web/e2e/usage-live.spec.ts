import { isolatedFixture } from './support/isolatedFixture';
import { expect, test, type APIRequestContext } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

const fixtureConfig = process.env.PAB_ADMIN_FIXTURE_CONFIG;
test.skip(!fixtureConfig, 'Requires the explicit isolated admin-console HTTPS fixture');
test.setTimeout(60000);
const password = 'isolated usage live fixture password';
const hour = Math.floor(Date.now() / 3600000) * 3600000;

async function account(context: APIRequestContext) {
  const username = `usage-live-${crypto.randomUUID().slice(0, 12)}`;
  const registration = await context.post('/api/account/register', { data: { username, password } });
  expect(registration.ok()).toBeTruthy(); const native = await registration.json();
  expect((await context.post('/api/web/session', { data: { username, password } })).ok()).toBeTruthy();
  return { username, id: native.user.id, token: native.access_token };
}

async function report(context: APIRequestContext, user: { id: string; token: string }, at: number, bytes: string) {
  const batch = { id: crypto.randomUUID(), user_id: user.id, hour_unix_ms: at, counters: { relay_upload_bytes: 0, relay_download_bytes: 0, connections: 1, connection_ms: 1000, uploaded_files: 1, downloaded_files: 0, uploaded_bytes: 0, downloaded_bytes: 0, failed_transfers: 0, cancelled_transfers: 0, incomplete: false } };
  const raw = JSON.stringify(batch).replace('"uploaded_bytes":0', `"uploaded_bytes":${bytes}`);
  for (let i = 0; i < 2; i++) expect((await context.post('/api/account/usage', { headers: { Authorization: `Bearer ${user.token}`, 'Content-Type': 'application/json' }, data: raw })).status()).toBe(204);
}

test('real usage report, half-open page and CSV agree without rounding or duplicate counting', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const context = await playwright.request.newContext({ baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } });
  try {
    const user = await account(context);
    await report(context, user, hour - 3600000, '17'); await report(context, user, hour, '9007199254740993');
    const query = `from=${hour}&until=${hour + 3600000}&interval=hour`;
    const summary = await (await context.get(`/api/web/usage?${query}`)).json();
    expect(summary.counters_exact.uploaded_bytes).toBe('9007199254740993');
    const previous = await (await context.get(`/api/web/usage?from=${hour - 3600000}&until=${hour}`)).json();
    expect(previous.counters_exact.uploaded_bytes).toBe('17');
    expect((await context.get('/api/web/usage/ranking?scope=all')).status()).toBe(403);
    await page.context().addCookies((await context.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto(`/usage?period=custom&${query}`);
    await expect(page.getByTitle('9007199254740993 B', { exact: true }).first()).toBeVisible();
    const pending = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export CSV', exact: true }).click();
    const file = await pending; expect(await file.failure()).toBeNull();
    const csv = await readFile((await file.path())!, 'utf8');
    expect(csv).toContain('"9007199254740993"'); expect(csv).toContain(`"${hour}","${hour + 3600000}"`);
    expect(csv.match(/"data",/g)).toHaveLength(1); expect(csv).toContain('"export_status"');
    expect(csv).not.toContain(user.token); expect(csv).not.toContain(password);
  } finally { await context.dispose(); }
});

test('real administrator ranking links keep account, population and window for CSV', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const options = { baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } };
  const admin = await playwright.request.newContext(options); const target = await playwright.request.newContext(options);
  try {
    const administrator = await account(admin); const user = await account(target);
    await report(target, user, hour, '9007199254740993');
    const config = path.resolve(fixtureConfig!);
    expect(config).toBe(isolatedFixture().config);
    execFileSync(isolatedFixture().executable, ['web-admin', administrator.username, '--config', config], { stdio: 'pipe' });
    const range = `from=${hour}&until=${hour + 3600000}&interval=hour&population=users`;
    const ranking = await (await admin.get(`/api/web/usage/ranking?scope=all&${range}&sort=uploaded_bytes&page_size=1`)).json();
    expect(ranking.items[0].counters_exact.uploaded_bytes).toBe('9007199254740993');
    await page.context().addCookies((await admin.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto(`/usage?period=custom&${range}&sort=uploaded_bytes`);
    await page.getByRole('button', { name: user.username, exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`user=${user.id}`));
    const url = new URL(page.url()); expect(url.searchParams.get('from')).toBe(String(hour)); expect(url.searchParams.get('until')).toBe(String(hour + 3600000)); expect(url.searchParams.get('population')).toBe('users');
    const pending = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export CSV', exact: true }).click();
    const file = await pending; const csv = await readFile((await file.path())!, 'utf8');
    expect(csv).toContain(user.username); expect(csv).not.toContain(administrator.username);
    expect(csv).toContain('"9007199254740993"');
  } finally { await Promise.all([admin.dispose(), target.dispose()]); }
});

test('real canceled usage requests leave the next CSV export writable', async ({ page, playwright, baseURL }) => {
  expect(baseURL).toBe('https://127.0.0.1:38623');
  const context = await playwright.request.newContext({ baseURL, ignoreHTTPSErrors: true, extraHTTPHeaders: { Origin: baseURL! } });
  try {
    const user = await account(context); await report(context, user, hour, '73');
    await page.context().addCookies((await context.storageState()).cookies);
    await page.addInitScript(() => localStorage.setItem('pab-web-language', 'en'));
    await page.goto(`/usage?period=custom&from=${hour}&until=${hour + 3600000}`);
    await expect(page.getByText('73 B', { exact: true }).first()).toBeVisible();
    await page.evaluate(async ({ from, until }) => {
      await Promise.all(Array.from({ length: 24 }, async () => {
        const controller = new AbortController(); const timer = setTimeout(() => controller.abort(), 5);
        try { await fetch(`/api/web/usage?from=${from}&until=${until}`, { signal: controller.signal }); }
        catch { /* Request cancellation is intentional. */ }
        finally { clearTimeout(timer); }
      }));
    }, { from: hour, until: hour + 3600000 });
    const pending = page.waitForEvent('download'); await page.getByRole('button', { name: 'Export CSV', exact: true }).click();
    const file = await pending; expect(await file.failure()).toBeNull();
    expect(await readFile((await file.path())!, 'utf8')).toContain('"73"');
  } finally { await context.dispose(); }
});
