import { expect, test } from '@playwright/test';

test('cancelling browser sign-in requires a second explicit confirmation', async ({ page }) => {
  await page.goto('http://127.0.0.1:1429/tests/browser-handoff.html');
  await expect(page.getByRole('dialog', { name: '浏览器等待你完成登录' })).toBeVisible();

  await page.getByRole('button', { name: '取消本次任务' }).click();
  await expect(page.getByRole('dialog', { name: '确定取消这次浏览器任务？' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.cancelCalls())).toBe(0);

  await page.getByRole('button', { name: '继续等待' }).click();
  await expect(page.getByRole('dialog', { name: '确定取消这次浏览器任务？' })).toHaveCount(0);
  await expect(page.getByRole('dialog', { name: '浏览器等待你完成登录' })).toBeVisible();
  expect(await page.evaluate(() => (window as any).fixture.cancelCalls())).toBe(0);

  await page.getByRole('button', { name: '取消本次任务' }).click();
  await page.getByRole('button', { name: '确认取消任务' }).click();
  await expect.poll(() => page.evaluate(() => (window as any).fixture.cancelCalls())).toBe(1);
  await expect(page.getByRole('dialog', { name: '浏览器等待你完成登录' })).toHaveCount(0);
});
