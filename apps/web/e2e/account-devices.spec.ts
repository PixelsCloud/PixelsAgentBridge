import { test, expect, type Page } from '@playwright/test';
import { designFixture, designAccountId, designDeviceId } from './support/designFixture';

async function fixture(page: Page, language = 'zh-CN') {
  const state = await designFixture(page, { language });
  const devices = Array.from({ length: 25 }, (_, index) => ({
    id: index === 0 ? designDeviceId : `device-${index}`, code: String(428358431 + index), name: `Associated device ${index + 1}`,
    online: index % 2 === 0, system: 'windows', os_name: 'Windows', agent_version: '1.2.48',
    client_version: index === 0 ? '1.3.27' : null, associated_user_id: designAccountId, associated_username: 'pixels-team',
    created_at: Date.now(), diagnostics: { runtime_reported: true, reported_at: Date.now() },
  }));
  const queries: URL[] = [];
  await page.route('**/api/web/accounts?*', route => route.fulfill({ json: { items: [
    { id: designAccountId, username: 'pixels-team', status: 'active', device_count: 25, saved_device_count: 25 },
    { id: 'empty-account', username: 'empty-account', status: 'active', device_count: 0, saved_device_count: 0 },
  ], total: 2, page: 1, page_size: 20 } }));
  let fail = false;
  await page.route('**/api/web/devices?*', route => {
    const url = new URL(route.request().url()); queries.push(url);
    if (fail) return route.fulfill({ status: 503, json: { code: 'server_error' } });
    const rows = url.searchParams.get('user') === 'empty-account' ? [] : devices;
    const current = Number(url.searchParams.get('page') ?? 1); const size = Number(url.searchParams.get('page_size') ?? 20);
    return route.fulfill({ json: { items: rows.slice((current - 1) * size, current * size), total: rows.length, page: current, page_size: size } });
  });
  await page.route(`**/api/web/devices/${designDeviceId}`, route => route.fulfill({ json: devices[0] }));
  await page.route('**/api/web/accounts/*/saved-devices?*', route => {
    const url = new URL(route.request().url()); queries.push(url);
    const rows = url.pathname.includes('/empty-account/') ? [] : devices.map((device, index) => ({ ...device, name: `Added device ${index + 1}`, alias: `Note ${index + 1}`, deleted: index % 2 === 1 }));
    const current = Number(url.searchParams.get('page') ?? 1); const size = Number(url.searchParams.get('page_size') ?? 20);
    return route.fulfill({ json: { items: rows.slice((current - 1) * size, current * size), total: rows.length, page: current, page_size: size } });
  });
  return { ...state, queries, setFailure: (value: boolean) => { fail = value; } };
}

for (const language of ['zh-CN', 'en']) test(`associated devices modal: pagination, dismissal and account isolation (${language})`, async ({ page }) => {
  const state = await fixture(page, language); const zh = language === 'zh-CN';
  await page.goto('/accounts');
  const trigger = page.getByRole('button', { name: `${zh ? '关联设备' : 'Associated devices'} · pixels-team`, exact: true });
  await expect(trigger).toHaveText('25'); await trigger.click();
  const modal = page.getByRole('dialog');
  await expect(modal.getByText('Associated device 1', { exact: true })).toBeVisible();
  await expect(modal.getByText('1.3.27', { exact: true })).toBeVisible();
  await expect(modal.getByText('1.2.48')).toHaveCount(0);
  await page.keyboard.press('Escape'); await expect(modal).toBeVisible();
  await page.locator('.ant-modal-wrap').click({ position: { x: 5, y: 5 }, force: true }); await expect(modal).toBeVisible();
  await modal.getByTitle('2', { exact: true }).click();
  await expect(modal.getByText('Associated device 25', { exact: true })).toBeVisible();
  expect(state.queries.at(-1)?.searchParams.get('page')).toBe('2');
  expect(state.queries.at(-1)?.searchParams.get('scope')).toBe('all');
  expect(state.queries.at(-1)?.searchParams.get('user')).toBe(designAccountId);
  await modal.getByRole('button', { name: zh ? /关\s*闭/ : 'Close', exact: true }).click();
  await page.getByRole('button', { name: `${zh ? '关联设备' : 'Associated devices'} · empty-account`, exact: true }).click();
  await expect(modal.getByText(zh ? '暂无数据' : 'No data', { exact: true })).toBeVisible();
  await expect(modal.getByText('Associated device 25', { exact: true })).toHaveCount(0);
  expect(state.queries.at(-1)?.searchParams.get('page')).toBe('1');
  expect(state.writes).toEqual([]);
});

test('device list and detail show account and installed version, without placeholder diagnostic fields', async ({ page }) => {
  await fixture(page); await page.goto('/devices');
  await expect(page.getByRole('columnheader', { name: '关联账号' })).toBeVisible();
  await expect(page.getByRole('row').filter({ hasText: 'Associated device 1' }).first()).toContainText('1.3.27');
  await expect(page.getByRole('row').filter({ hasText: 'Associated device 2' }).first()).toContainText('未上报');
  await page.goto(`/devices/${designDeviceId}`);
  await expect(page.getByText('1.3.27', { exact: true })).toBeVisible();
  await expect(page.getByText('权限状态', { exact: true })).toHaveCount(0);
  await expect(page.getByText('兼容状态', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'pixels-team', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/accounts/${designAccountId}$`));
});

test('failed device read can be retried without closing the modal', async ({ page }) => {
  const state = await fixture(page); state.setFailure(true); await page.goto('/accounts');
  await page.getByRole('button', { name: '关联设备 · pixels-team', exact: true }).click();
  const modal = page.getByRole('dialog'); await expect(modal.getByRole('alert')).toBeVisible();
  state.setFailure(false); await modal.getByRole('button', { name: /重\s*试/, exact: true }).click();
  await expect(modal.getByText('Associated device 1', { exact: true })).toBeVisible();
});

for (const language of ['zh-CN', 'en']) test(`added devices include removed entries and paginate separately (${language})`, async ({ page }) => {
  const state = await fixture(page, language); const zh = language === 'zh-CN';
  await page.goto('/accounts');
  const label = zh ? '添加的设备' : 'Added devices';
  await expect(page.getByRole('columnheader', { name: label, exact: true })).toBeVisible();
  const button = page.getByRole('button', { name: `${label} · pixels-team`, exact: true });
  await expect(button).toHaveText('25'); await button.click();
  const modal = page.getByRole('dialog');
  await expect(modal.getByRole('row').filter({ hasText: 'Added device 2' }).first()).toContainText(zh ? '已移除' : 'Removed');
  await expect(modal.getByText('Note 1', { exact: true })).toBeVisible();
  await page.keyboard.press('Escape'); await expect(modal).toBeVisible();
  await page.locator('.ant-modal-wrap').click({ position: { x: 5, y: 5 }, force: true }); await expect(modal).toBeVisible();
  await modal.getByTitle('2', { exact: true }).click();
  await expect(modal.getByText('Added device 25', { exact: true })).toBeVisible();
  expect(state.queries.at(-1)?.pathname).toBe(`/api/web/accounts/${designAccountId}/saved-devices`);
  expect(state.queries.at(-1)?.searchParams.get('page')).toBe('2');
  await modal.getByRole('button', { name: zh ? /关\s*闭/ : 'Close', exact: true }).click();
  await page.getByRole('button', { name: `${label} · empty-account`, exact: true }).click();
  await expect(modal.getByText(zh ? '暂无数据' : 'No data', { exact: true })).toBeVisible();
  await expect(modal.getByText('Added device 25', { exact: true })).toHaveCount(0);
  expect(state.writes).toEqual([]);
});
