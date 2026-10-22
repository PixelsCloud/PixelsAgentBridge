import { expect, test } from '@playwright/test';
import { designFixture } from './support/designFixture';

test('slow initial usage reads keep labelled metric frames visible without animated skeletons', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/api/web/usage?*', async route => {
    const url = new URL(route.request().url());
    await gate;
    await route.fulfill({ json: { from: Number(url.searchParams.get('from')), until: Number(url.searchParams.get('until')), interval: 'hour', timezone: 'UTC', updated_at: Date.now(), counters_exact: { relay_upload_bytes: '1024' }, points: [] } }).catch(() => {});
  });
  await page.goto('/usage');
  const card = page.locator('.usage-metric-card').filter({ hasText: 'Relay upload' });
  await expect(card).toBeVisible();
  const value = await card.locator('.ant-typography').nth(1).elementHandle();
  try {
    await expect(page.locator('.usage-metrics-grid')).toHaveAttribute('aria-busy', 'true');
    await expect(page.locator('.usage-metrics-grid .ant-skeleton')).toHaveCount(0);
    await expect(card).toContainText('—');
    await page.getByRole('combobox', { name: 'Usage period', exact: true }).selectOption('month');
    await page.getByRole('combobox', { name: 'Usage period', exact: true }).selectOption('7');
    expect(await value!.evaluate(node => node.isConnected)).toBe(true);
    await expect(page.locator('.usage-metrics-grid .ant-skeleton')).toHaveCount(0);
  } finally { release(); }
  await expect(card).toContainText('1 KiB');
  expect(await value!.evaluate(node => node.isConnected)).toBe(true);
});

test('changing usage periods preserves metric nodes until the selected totals arrive', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  await page.clock.setFixedTime(new Date('2026-10-22T09:00:00Z'));
  let delayed = false;
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/api/web/usage?*', async route => {
    const url = new URL(route.request().url());
    const from = Number(url.searchParams.get('from'));
    const until = Number(url.searchParams.get('until'));
    const current = until - from > 2 * 86400000;
    if (delayed) await gate;
    await route.fulfill({ json: {
      from, until, interval: 'hour', timezone: 'UTC', updated_at: until - 1,
      counters_exact: { relay_upload_bytes: current ? '1048576' : '1024' }, points: [],
    } }).catch(() => {});
  });
  await page.goto('/usage?period=today');
  const card = page.locator('.usage-metric-card').filter({ hasText: 'Relay upload' });
  await expect(card).toContainText('1 KiB');
  const value = await card.locator('.ant-typography').nth(1).elementHandle();
  delayed = true;
  await page.getByRole('combobox', { name: 'Usage period', exact: true }).selectOption('month');
  try {
    await expect(page.getByRole('button', { name: 'Export CSV', exact: true })).toBeDisabled();
    await page.waitForTimeout(250);
    expect(await value!.evaluate(node => node.isConnected)).toBe(true);
    await expect(page.locator('.usage-metrics-grid .ant-skeleton')).toHaveCount(0);
    await expect(card).toContainText('1 KiB');
    await expect(page.locator('.usage-metrics-grid')).toHaveAttribute('aria-busy', 'true');
  } finally { release(); }
  await expect(card).toContainText('1 MiB');
  await expect(page.locator('.usage-metrics-grid')).toHaveAttribute('aria-busy', 'false');
  expect(await value!.evaluate(node => node.isConnected)).toBe(true);
  await expect(page.getByRole('button', { name: 'Export CSV', exact: true })).toBeEnabled();
});

test('rapid period changes ignore obsolete replies and keep export tied to the final selection', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  await page.clock.setFixedTime(new Date('2026-10-22T09:00:00Z'));
  let releaseMonth!: () => void; let releaseWeek!: () => void;
  const month = new Promise<void>(resolve => { releaseMonth = resolve; });
  const week = new Promise<void>(resolve => { releaseWeek = resolve; });
  const reads: number[] = [];
  const exports: string[] = [];
  await page.route('**/api/web/usage?*', async route => {
    const url = new URL(route.request().url());
    const from = Number(url.searchParams.get('from')); const until = Number(url.searchParams.get('until'));
    const days = (until - from) / 86400000;
    reads.push(days);
    if (days > 10) await month;
    else if (days > 2) await week;
    await route.fulfill({ json: {
      from, until, interval: 'hour', timezone: 'UTC', updated_at: until - 1,
      counters_exact: { relay_upload_bytes: days > 10 ? '1048576' : days > 2 ? '2097152' : '1024' }, points: [],
    } }).catch(() => {});
  });
  await page.route('**/api/web/usage/export?*', async route => {
    exports.push(route.request().url());
    await route.fulfill({ contentType: 'text/csv', headers: { 'content-disposition': 'attachment; filename="usage.csv"' }, body: 'fixture' });
  });
  await page.goto('/usage?period=today');
  const card = page.locator('.usage-metric-card').filter({ hasText: 'Relay upload' });
  await expect(card).toContainText('1 KiB');
  const value = await card.locator('.ant-typography').nth(1).elementHandle();
  const period = page.getByRole('combobox', { name: 'Usage period', exact: true });
  await period.selectOption('month');
  await expect.poll(() => reads.some(days => days > 10)).toBe(true);
  await period.selectOption('7');
  await expect.poll(() => reads.some(days => days === 7)).toBe(true);
  try {
    releaseMonth(); await page.waitForTimeout(150);
    await expect(card).toContainText('1 KiB');
    await expect(page.getByRole('button', { name: 'Export CSV', exact: true })).toBeDisabled();
    expect(await value!.evaluate(node => node.isConnected)).toBe(true);
  } finally { releaseMonth(); releaseWeek(); }
  await expect(card).toContainText('2 MiB');
  expect(await value!.evaluate(node => node.isConnected)).toBe(true);
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Export CSV', exact: true }).click();
  await download;
  expect(exports).toHaveLength(1);
  const url = new URL(exports[0]);
  expect(Number(url.searchParams.get('until')) - Number(url.searchParams.get('from'))).toBe(7 * 86400000);
});

test('a fast previous-period reply is never compared against totals from another selection', async ({ page }) => {
  await designFixture(page, { language: 'en' });
  await page.clock.setFixedTime(new Date('2026-10-22T09:00:00Z'));
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  let delayed = false; let previousArrived = false;
  await page.route('**/api/web/usage?*', async route => {
    const url = new URL(route.request().url());
    const from = Number(url.searchParams.get('from')); const until = Number(url.searchParams.get('until'));
    const current = until === Date.parse('2026-10-22T10:00:00Z');
    if (delayed && current) await gate;
    const bytes = delayed ? current ? '1048576' : '2097152' : '1024';
    await route.fulfill({ json: { from, until, interval: 'hour', timezone: 'UTC', updated_at: until - 1, counters_exact: { relay_upload_bytes: bytes }, points: [] } }).catch(() => {});
    if (delayed && !current) previousArrived = true;
  });
  await page.goto('/usage');
  const card = page.locator('.usage-metric-card').filter({ hasText: 'Relay upload' });
  await expect(card).toContainText('Previous period: 1 KiB');
  delayed = true;
  await page.getByRole('combobox', { name: 'Usage period', exact: true }).selectOption('month');
  try {
    await expect.poll(() => previousArrived).toBe(true);
    await expect(card).toContainText('1 KiB');
    await expect(card).not.toContainText('2 MiB');
  } finally { release(); }
  await expect(card).toContainText('1 MiB');
  await expect(card).toContainText('Previous period: 2 MiB');
});
