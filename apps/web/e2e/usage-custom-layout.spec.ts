import { expect, test } from '@playwright/test';

for (const [language, theme, width] of [['en', 'light', 360], ['en', 'dark', 390], ['zh-CN', 'light', 360], ['zh-CN', 'dark', 390]] as const) {
  test(`custom usage inputs fit ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.addInitScript(({ language, theme }) => { localStorage.setItem('pab-web-language', language); localStorage.setItem('pab-web-theme', theme); }, { language, theme });
    await page.routeWebSocket('**/api/web/events', () => {});
    await page.route('**/api/web/**', async route => {
      const url = new URL(route.request().url()); const endpoint = url.pathname.replace('/api/web', '');
      const data = endpoint === '/config' ? { registration_enabled: false, version: 'fixture' }
        : endpoint === '/session' ? { id: 'layout-user', username: 'Layout user', server_admin: false }
        : endpoint === '/traffic' ? { user_mbps: 10 }
        : { from: Number(url.searchParams.get('from')), until: Number(url.searchParams.get('until')), interval: 'hour', counters: {}, counters_exact: {}, points: [], updated_at: null };
      await route.fulfill({ json: data });
    });
    const hour = Math.floor(Date.now() / 3600000) * 3600000;
    await page.goto(`/usage?period=custom&from=${hour - 3600000}&until=${hour}`);
    await expect(page.locator('input[type="datetime-local"]')).toHaveCount(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
  });
}
