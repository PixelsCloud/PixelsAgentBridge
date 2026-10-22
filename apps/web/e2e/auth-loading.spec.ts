import { expect, test } from '@playwright/test';
import { designFixture } from './support/designFixture';

for (const reducedMotion of ['reduce', 'no-preference'] as const) for (const width of [360, 1440]) for (const register of [false, true]) {
  test(`authentication pending state stays stable ${register ? 'register' : 'login'} ${width} ${reducedMotion}`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion }); await page.setViewportSize({ width, height: 1000 });
    await designFixture(page, { signedOut: true, language: 'en' });
    let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; }); let requests = 0;
    await page.route(`**/api/web/${register ? 'register' : 'session'}`, async route => {
      if (route.request().method() === 'GET') return route.fallback();
      requests++; await gate;
      await route.fulfill({ status: 500, json: { code: 'server_error' } }).catch(() => {});
    });
    await page.goto('/');
    if (register) await page.getByRole('button', { name: 'Create an account', exact: true }).click();
    await page.getByLabel('Username', { exact: true }).fill('pending-fixture');
    await page.getByLabel('Password', { exact: true }).fill('fixture-only-password');
    if (register) await page.getByLabel('Confirm password', { exact: true }).fill('fixture-only-password');
    const button = page.locator('.auth-submit'); const node = await button.elementHandle();
    await button.click();
    try {
      await expect(button).toBeDisabled(); await expect(button).toHaveAttribute('aria-busy', 'true');
      await expect(button).toHaveText(register ? 'Creating account…' : 'Signing in…');
      await page.locator('form').evaluate(form => { for (let i = 0; i < 8; i++) (form as HTMLFormElement).requestSubmit(); });
      // The requested default button transition may run once on submit. Check
      // stability after it settles, while still rejecting a looping animation.
      await expect.poll(() => button.evaluate(element => element.getAnimations().filter(animation => animation.playState === 'running').length)).toBe(0);
      const variants = await button.evaluate(element => new Promise<string[]>(resolve => {
        const samples = new Set<string>(); let frames = 0;
        const sample = () => {
          const rect = element.getBoundingClientRect(); const css = getComputedStyle(element);
          samples.add(JSON.stringify([rect.x, rect.y, rect.width, rect.height, css.opacity, element.textContent, css.backgroundColor]));
          if (++frames === 60) resolve([...samples]); else requestAnimationFrame(sample);
        }; requestAnimationFrame(sample);
      }));
      expect(variants).toHaveLength(1); expect(requests).toBe(1);
      expect(await node!.evaluate(element => element.isConnected)).toBe(true);
      await expect(button.locator('.anticon-loading')).toHaveCount(0);
    } finally { release(); }
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(button).toBeEnabled(); await expect(button).toHaveAttribute('aria-busy', 'false');
    await button.click(); await expect.poll(() => requests).toBe(2);
    await expect(button).toBeEnabled();
  });
}

test('startup spinner uses its normal animation duration', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await designFixture(page, { signedOut: true, language: 'en' });
  let release!: () => void; const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route('**/api/web/config', async route => { await gate; await route.fallback(); });
  await page.goto('/');
  try {
    const startup = page.locator('.full-center .ant-spin'); await expect(startup).toBeVisible();
    const durations = await startup.evaluate(element => element.getAnimations({ subtree: true }).filter(animation => animation.playState === 'running').map(animation => Number(animation.effect?.getTiming().duration)));
    expect(durations.length).toBeGreaterThan(0);
    expect(durations.every(duration => duration >= 100)).toBe(true);
  } finally { release(); }
  await expect(page.locator('.auth-card')).toBeVisible();
});
