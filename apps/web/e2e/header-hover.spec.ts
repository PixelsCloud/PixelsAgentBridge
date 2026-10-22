import { expect, test } from '@playwright/test';
import { designFixture } from './support/designFixture';

test.use({ launchOptions: { ignoreDefaultArgs: ['--hide-scrollbars'] } });

for (const path of ['/account', '/usage', '/settings']) test(`sign-out hover preserves page geometry on ${path}`, async ({ page }) => {
  await designFixture(page, { language: 'en' });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(path);
  if (path === '/usage') await expect(page.locator('.usage-metrics-grid')).toHaveAttribute('aria-busy', 'false');
  if (path === '/account') await expect(page.locator('.account-password input')).toHaveCount(3);
  await expect(page.locator('main .ant-skeleton, main .ant-spin-spinning')).toHaveCount(0);
  const button = page.getByRole('button', { name: 'Sign out', exact: true });
  await expect(button).toHaveAttribute('title', 'Sign out');
  await page.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
  const before = await page.evaluate(() => {
    const box = document.querySelector('.logout-button')!.getBoundingClientRect();
    return { x: box.x, y: box.y, client: document.documentElement.clientWidth, width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight };
  });
  const box = (await button.boundingBox())!;
  await page.mouse.move(box.x + box.width - 2, box.y + box.height / 2);
  const changed = await page.evaluate(() => new Promise<unknown[]>(resolve => {
    const initial = document.querySelector('.logout-button')!.getBoundingClientRect();
    const width = document.documentElement.clientWidth; const height = document.documentElement.scrollHeight;
    const changed: unknown[] = []; let frames = 0;
    const sample = () => {
      const box = document.querySelector('.logout-button')!.getBoundingClientRect();
      if (box.x !== initial.x || box.y !== initial.y || document.documentElement.clientWidth !== width || document.documentElement.scrollHeight !== height) changed.push({ x: box.x, y: box.y, width: document.documentElement.clientWidth, height: document.documentElement.scrollHeight });
      if (++frames === 90) resolve(changed); else requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  }));
  expect(changed).toEqual([]);
  await expect(page.locator('.ant-tooltip')).toHaveCount(0);
  const after = await page.evaluate(() => {
    const box = document.querySelector('.logout-button')!.getBoundingClientRect();
    return { x: box.x, y: box.y, client: document.documentElement.clientWidth, width: document.documentElement.scrollWidth, height: document.documentElement.scrollHeight };
  });
  expect(after).toEqual(before);
});
