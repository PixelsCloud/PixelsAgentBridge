import { expect, test, type WebSocketRoute } from '@playwright/test';
import { designFixture } from './support/designFixture';

test('continuous notifications cannot cancel and restart a slow initial read', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  let socket: WebSocketRoute | undefined; let reads = 0; let aborted = 0;
  await page.routeWebSocket('**/api/web/events', value => { socket = value; });
  page.on('requestfailed', request => { if (new URL(request.url()).pathname === '/api/web/overview') aborted++; });
  await page.route('**/api/web/overview?*', async route => {
    reads++; await new Promise(resolve => setTimeout(resolve, 900));
    await route.fulfill({ json: { total: 48, online: 36, offline: 12, accounts: 4 } }).catch(() => {});
  });
  await page.goto('/'); await expect.poll(() => !!socket).toBe(true);
  let sequence = 0;
  const timer = setInterval(() => socket!.send(JSON.stringify({ type: 'refresh', sequence: ++sequence })), 200);
  try {
    await expect(page.locator('.metric-value').first()).toContainText('48', { timeout: 2000 });
    const metric = await page.locator('.metric-value').first().elementHandle();
    await page.waitForTimeout(1200);
    expect(aborted).toBe(0);
    expect(reads).toBeLessThanOrEqual(5);
    expect(await metric!.evaluate(node => node.isConnected)).toBe(true);
    await expect(page.locator('.overview-stats .ant-skeleton')).toHaveCount(0);
  } finally { clearInterval(timer); }
});

test('refreshes during an active read queue one subsequent read with the latest data', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  let socket: WebSocketRoute | undefined; let reads = 0; let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.routeWebSocket('**/api/web/events', value => { socket = value; });
  await page.route('**/api/web/overview?*', async route => {
    const total = ++reads === 1 ? 48 : 99;
    if (reads === 1) await gate;
    await route.fulfill({ json: { total, online: 36, offline: total - 36, accounts: 4 } }).catch(() => {});
  });
  await page.goto('/'); await expect.poll(() => reads).toBe(1); await expect.poll(() => !!socket).toBe(true);
  try {
    for (let sequence = 1; sequence <= 3; sequence++) {
      socket!.send(JSON.stringify({ type: 'refresh', sequence }));
      await page.waitForTimeout(200);
    }
    expect(reads).toBe(1);
  } finally { release(); }
  await expect(page.locator('.metric-value').first()).toContainText('99');
  expect(reads).toBe(2);
});

test('retrying a failed background read does not flash an old healthy snapshot', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  let socket: WebSocketRoute | undefined; let reads = 0; let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.routeWebSocket('**/api/web/events', value => { socket = value; });
  await page.route('**/api/web/overview?*', async route => {
    const current = ++reads;
    if (current === 3) await gate;
    await route.fulfill(current === 2 || current === 3
      ? { status: 503, json: { code: 'server_error' } }
      : { json: { total: current === 1 ? 48 : 99, online: 36, offline: 12, accounts: 4 } }).catch(() => {});
  });
  await page.goto('/'); await expect(page.locator('.metric-value').first()).toContainText('48');
  await expect.poll(() => !!socket).toBe(true);
  socket!.send(JSON.stringify({ type: 'refresh', sequence: 1 }));
  await expect(page.locator('.ant-alert-error')).toBeVisible();
  socket!.send(JSON.stringify({ type: 'refresh', sequence: 2 }));
  try {
    await expect.poll(() => reads).toBe(3);
    await expect(page.locator('.ant-alert-error')).toBeVisible();
    await expect(page.locator('.metric-value').first()).toHaveText('—');
    socket!.send(JSON.stringify({ type: 'refresh', sequence: 3 }));
    await page.waitForTimeout(200);
  } finally { release(); }
  await expect(page.locator('.metric-value').first()).toContainText('99');
  await expect(page.locator('.ant-alert-error')).toHaveCount(0);
});
