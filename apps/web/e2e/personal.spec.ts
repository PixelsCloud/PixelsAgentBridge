import { test, expect } from '@playwright/test';
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

test('personal device proof, two-client catalog sync, usage, unlink and account isolation', async ({ page, browser }) => {
  test.setTimeout(120000);
  const root = resolve(import.meta.dirname, '../../..');
  const origin = 'https://localhost:38443';
  const username = `personal-${Date.now()}`; const password = 'personal-test-password';
  const response = await page.request.post(`${origin}/api/account/register`, { data: { username, password } });
  expect(response.ok()).toBeTruthy(); const native = await response.json();
  const bearer = { Authorization: `Bearer ${native.access_token}` };
  expect((await page.request.post('/api/web/session', { headers: { Origin: origin }, data: { username, password } })).ok()).toBeTruthy();
  const child = spawn(resolve(root, `target/debug/examples/web-device-fixture${process.platform === 'win32' ? '.exe' : ''}`), [`wss://localhost:38443/control`, resolve(root, '.build/web-test/cert.pem'), 'Personal E2E machine'], { windowsHide: true });
  const lines = createInterface({ input: child.stdout }); const messages: Record<string, any>[] = []; let failure = '';
  lines.on('line', line => { try { messages.push(JSON.parse(line)); } catch { failure += line; } });
  child.stderr.on('data', chunk => { failure += String(chunk); });
  const next = async (field: string, value: unknown) => {
    await expect.poll(() => failure || messages.some(item => item[field] === value), { timeout: 20000 }).toBe(true);
    return messages.splice(messages.findIndex(item => item[field] === value), 1)[0];
  };
  const second = await browser.newContext({ ignoreHTTPSErrors: true, locale: 'en-US' });
  try {
    const device = await next('ready', true);
    const path = `${origin}/api/account/devices/${device.device_id}/association`;
    const challenge = await (await page.request.post(`${path}-challenge`, { headers: bearer, data: { action: 'automatic' } })).json();
    child.stdin.write(JSON.stringify({ action: 'sign_association', challenge: challenge.challenge }) + '\n');
    const proof = await next('done', 'sign_association');
    expect((await page.request.put(path, { headers: bearer, data: proof.proof })).ok()).toBeTruthy();
    await page.goto('/devices'); await expect(page.getByRole('heading', { name: 'My devices', exact: true })).toBeVisible();
    await expect(page.getByText('Personal E2E machine', { exact: true })).toBeVisible();
    expect((await page.request.get('/api/web/devices?scope=all')).status()).toBe(403);
    expect((await page.request.get('/api/web/usage?scope=all')).status()).toBe(403);
    expect((await page.request.post(`${origin}/api/account/saved-devices/import`, { headers: bearer, data: { device_ref: { device_id: device.device_id, tenant_id: device.tenant_id }, code: String(device.device_code), name: 'Imported remote', system: 'windows', alias: 'E2E alias' } })).ok()).toBeTruthy();
    const hour = Math.floor(Date.now() / 3600000) * 3600000;
    const counters = { relay_upload_bytes: 0, relay_download_bytes: 0, connections: 1, connection_ms: 1000, uploaded_files: 1, downloaded_files: 0, uploaded_bytes: 4096, downloaded_bytes: 0, failed_transfers: 0, cancelled_transfers: 0, incomplete: false };
    const batch = { id: crypto.randomUUID(), user_id: native.user.id, hour_unix_ms: hour, counters };
    for (let i = 0; i < 2; i++) expect((await page.request.post(`${origin}/api/account/usage`, { headers: bearer, data: batch })).status()).toBe(204);
    await second.addCookies((await page.context().storageState()).cookies);
    const other = await second.newPage(); await other.goto(`${origin}/saved-devices`);
    await expect(other.getByText('E2E alias', { exact: true })).toBeVisible();
    await page.goto('/saved-devices'); await expect(page.getByText('Reconnect to verify access', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Personal alias', exact: true }).click();
    await page.getByRole('dialog').getByRole('textbox').fill('Renamed from client A');
    await page.getByRole('dialog').getByRole('button', { name: 'OK', exact: true }).click();
    await expect(other.getByText('Renamed from client A', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Remove', exact: true }).click();
    await page.mouse.click(4, 4); await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('dialog').getByRole('button', { name: 'OK', exact: true }).click();
    await expect(other.getByText('Renamed from client A', { exact: true })).toHaveCount(0);
    await page.goto('/usage'); await expect(page.getByRole('heading', { name: 'My usage', exact: true })).toBeVisible();
    await expect(page.getByText('4.0 KiB', { exact: true }).first()).toBeVisible();
    await page.goto(`/devices/${device.device_id}`);
    await page.getByRole('button', { name: 'Unlink account', exact: true }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'OK', exact: true }).click();
    await expect(page.getByText('Personal E2E machine', { exact: true })).toHaveCount(0);
    expect((await (await page.request.post(`${path}-challenge`, { headers: bearer, data: { action: 'automatic' } })).json()).status).toBe('unlinked');
    // A separate account receives neither this user's saved metadata nor usage.
    const newUser = await second.request.post(`${origin}/api/web/register`, { headers: { Origin: origin }, data: { username: `other-${Date.now()}`, password } });
    expect(newUser.ok()).toBeTruthy();
    const usage = await (await second.request.get(`${origin}/api/web/usage`)).json(); expect(usage.counters.uploaded_bytes).toBe(0);
    expect((await (await second.request.get(`${origin}/api/web/saved-devices`)).json()).items).toEqual([]);
  } finally { child.kill(); lines.close(); await second.close(); }
});
