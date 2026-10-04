import { test, expect } from '@playwright/test';

test('production assets, deep links and API boundaries', async ({ request, page }) => {
  const root = await request.get('/');
  expect(root.status()).toBe(200);
  expect(root.headers()['cache-control']).toBe('no-store');
  expect(root.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
  const html = await root.text();
  const script = html.match(/src="([^"]+\.js)"/)?.[1];
  expect(script).toBeTruthy();
  const asset = await request.get(script!);
  expect(asset.status()).toBe(200);expect(asset.headers()['cache-control']).toContain('immutable');
  expect((await request.get('/assets/does-not-exist.js')).status()).toBe(404);
  for (const path of ['/api','/api/web/tasks','/api/web/unknown']) {
    const response = await request.get(path);expect(response.status()).toBe(404);expect((await response.json()).code).toBe('not_found');
  }
  await page.goto('/devices/deep-link');
  await expect(page.getByRole('heading',{name:'Sign in',exact:true})).toBeVisible();
});
