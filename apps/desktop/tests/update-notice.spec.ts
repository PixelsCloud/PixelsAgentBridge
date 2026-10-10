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
