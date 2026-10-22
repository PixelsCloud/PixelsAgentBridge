import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';
import { designAccountId, designDeviceId, designFixture } from './support/designFixture';

async function fixture(page: Page) {
  await page.clock.install();
  const state = await designFixture(page, { language: 'en' });
  let socket: WebSocketRoute;
  let sequence = 0;
  await page.routeWebSocket('**/api/web/events', value => { socket = value; });
  return {
    ...state,
    send: () => socket.send(JSON.stringify({ type: 'refresh', sequence: ++sequence })),
  };
}

for (const [route, resource, initial] of [
  ['/', '/overview', 1], ['/devices', '/devices', 1],
  [`/devices/${designDeviceId}`, `/devices/${designDeviceId}`, 1],
  ['/accounts', '/accounts', 1], [`/accounts/${designAccountId}`, `/accounts/${designAccountId}`, 1],
  ['/relay', '/relays', 1], ['/usage', '/usage', 2],
  ['/settings', '/service', 1], ['/saved-devices', '/saved-devices', 1],
] as const) {
  test(`${route} loads once and preserves normal cadence without duplicate burst reads`, async ({ page }) => {
    const state = await fixture(page); await page.goto(route);
    const count = () => state.reads.filter(path => path === resource).length;
    await expect.poll(count).toBe(initial);
    await page.clock.pauseAt(await page.evaluate(() => Date.now()) + 500);
    const metric = route === '/' ? await page.locator('.metric-value').first().elementHandle() : null;
    for (let i = 0; i < 3; i++) {
      for (let n = 0; n < 20; n++) state.send();
      await page.clock.runFor(10_000);
      await expect.poll(count).toBe(route === '/relay' ? initial + (i + 1) * 2 : initial * (i + 2));
    }
    if (metric) expect(await metric.evaluate(node => node.isConnected)).toBe(true);
    expect(state.writes).toHaveLength(0);
  });
}

test('hidden pages pause reads and refresh once on return; manual refresh stays immediate', async ({ page }) => {
  const state = await fixture(page); await page.goto('/devices');
  const count = () => state.reads.filter(path => path === '/devices').length;
  await expect.poll(count).toBe(1);
  await page.clock.pauseAt(await page.evaluate(() => Date.now()) + 500);
  await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); document.dispatchEvent(new Event('visibilitychange')); });
  for (let i = 0; i < 9; i++) { state.send(); await page.clock.runFor(10_000); }
  expect(count()).toBe(1);
  await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' }); document.dispatchEvent(new Event('visibilitychange')); });
  await expect.poll(count).toBe(2);
  await page.getByRole('button', { name: 'Refresh', exact: true }).first().click();
  await expect.poll(count).toBe(3);
});

test('audit snapshots remain stable without change notices until manually refreshed', async ({ page }) => {
  const state = await fixture(page); await page.goto('/audit');
  await expect(page).toHaveURL(/cursor=preview-snapshot/);
  const count = () => state.reads.filter(path => path === '/audit').length;
  await expect.poll(count).toBe(2);
  await page.clock.pauseAt(await page.evaluate(() => Date.now()) + 500);
  for (let i = 0; i < 6; i++) { state.send(); await page.clock.runFor(10_000); }
  expect(count()).toBe(2);
  await expect(page.locator('main .ant-alert')).toHaveCount(0);
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect.poll(count).toBeGreaterThan(2);
});

test('operation polling pauses while hidden and stops after completion', async ({ page }) => {
  const state = await fixture(page);
  const id = '51000000-0000-4000-8000-000000000003';
  let phase = 'waiting'; let reads = 0;
  await page.addInitScript(({ key, id }) => localStorage.setItem(key, JSON.stringify({ id, unconfirmed: false })), { key: `pab-relay-operation:${designAccountId}`, id });
  await page.route(`**/api/web/relay-operations/${id}`, route => {
    reads++;
    return route.fulfill({ json: { id, phase, node_id: 'ap-east-primary', action: 'maintenance', actor: 'pixels-team', started_at_unix_ms: Date.now(), deadline_unix_ms: Date.now() + 30000, completed_at_unix_ms: null, server_now_unix_ms: Date.now(), capacity_sampled_at_unix_ms: Date.now(), remaining_capacity_mbps: null, error_code: null, allow_outage: false } });
  });
  await page.goto('/relay'); await expect.poll(() => reads).toBe(1);
  await expect(page.getByTestId('relay-operation')).toContainText('Waiting for transition');
  await page.clock.pauseAt(await page.evaluate(() => Date.now()) + 500);
  await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); document.dispatchEvent(new Event('visibilitychange')); });
  state.send(); await page.clock.runFor(10_000); expect(reads).toBe(1);
  await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' }); document.dispatchEvent(new Event('visibilitychange')); });
  await expect.poll(() => reads).toBe(2);
  phase = 'completed'; state.send(); await page.clock.runFor(5000);
  await expect.poll(() => reads).toBe(3);
  await expect(page.getByTestId('relay-operation')).toContainText('Administrative action completed');
  state.send(); await page.clock.runFor(15_000); expect(reads).toBe(3);
  expect(state.writes).toHaveLength(0);
});
