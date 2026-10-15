import { expect, test } from '@playwright/test';

test('a discovered release is visible and opens the update page', async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/update-notice.html');
  await expect.poll(() => page.evaluate(() => (window as any).fixture.listeners())).toBe(1);
  await page.evaluate(() => (window as any).fixture.emit({ version: '1.2.79' }));
  await expect(page.getByRole('status')).toContainText('新版本 V1.2.79');
  await page.getByRole('button', { name: '新版本 V1.2.79，查看更新' }).click();
  await expect(page.locator('body')).toHaveAttribute('data-open', 'true');
  await page.evaluate(() => (window as any).fixture.emit(null));
  await expect(page.getByRole('status')).toHaveCount(0);
});

test('download automatically starts installation only after verification completes', async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/update-notice.html?panel=update&delayDownload');
  await expect.poll(() => page.evaluate(() => (window as any).fixture.listeners())).toBe(2);
  await page.getByRole('button', { name: '下载并安装', exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).fixture.finishDownload)).toBe('function');
  await page.evaluate(() => (window as any).fixture.progress());
  await expect(page.getByRole('button', { name: '检查更新' })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: string) => c === 'install_update'))).toEqual([]);
  await page.evaluate(() => (window as any).fixture.finishDownload());
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: string) => c.endsWith('_update')))).toEqual(['download_update', 'install_update']);
  await expect(page.getByRole('button', { name: '正在启动安装' })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  // An already-started installation cannot be launched a second time.
  await page.getByRole('button', { name: '正在启动安装' }).evaluate((button: HTMLButtonElement) => button.click());
  expect(await page.evaluate(() => (window as any).fixture.calls.filter((c: string) => c === 'install_update').length)).toBe(1);
});

test('a failed download or verification never launches the installer', async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/update-notice.html?panel=update&downloadError');
  await page.getByRole('button', { name: '下载并安装', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('checksum does not match');
  expect(await page.evaluate(() => (window as any).fixture.calls.includes('install_update'))).toBe(false);
  await expect(page.getByRole('button', { name: '检查更新' })).toBeEnabled();
});

test('installer launch failure allows retry with revalidation and no confirmation dialog', async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/update-notice.html?panel=update&installError');
  await page.getByRole('button', { name: '下载并安装', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Could not start the installer');
  await page.getByRole('button', { name: '重试安装', exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.calls.filter((c: string) => c.endsWith('_update')))).toEqual(['download_update', 'install_update', 'download_update', 'install_update']);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByRole('dialog')).toHaveCount(0);
});
