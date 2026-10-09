import { test, expect } from '@playwright/test';

test('GitHub sign-in needs no registration fields and handles cancelled authorization', async ({ page }) => {
  const config = await (await page.request.get('/api/web/config')).json();
  test.skip(!config.github_enabled, 'GitHub is not configured in this fixture');
  let authorization: URL | undefined;
  // Do not visit GitHub or expose a deployment identifier in test artifacts.
  await page.route('https://github.com/login/oauth/authorize?*', async route => {
    authorization = new URL(route.request().url());
    await route.fulfill({ contentType: 'text/html', body: '<p>OAuth provider fixture</p>' });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Continue with GitHub', exact: true }).click();
  await expect.poll(() => Boolean(authorization)).toBe(true);
  expect(authorization!.searchParams.get('code_challenge_method')).toBe('S256');
  expect(authorization!.searchParams.get('code_challenge')).toBeTruthy();
  const callback = new URL(authorization!.searchParams.get('redirect_uri')!);
  callback.searchParams.set('state', authorization!.searchParams.get('state')!);
  callback.searchParams.set('error', 'access_denied');
  await page.goto(callback.href);
  await expect(page).toHaveURL(/github_error=github_cancelled/);
  await expect(page.getByRole('heading', { name: 'Sign in', exact: true })).toBeVisible();
  expect((await page.request.get('/api/web/session')).status()).toBe(401);
  await page.goto(callback.href);
  await expect(page).toHaveURL(/github_error=github_expired/);
});

test('GitHub login boundary rejects cross-site requests and unexpected native return addresses', async ({ request }) => {
  const config = await (await request.get('/api/web/config')).json();
  test.skip(!config.github_enabled, 'GitHub is not configured in this fixture');
  expect((await request.post('/api/web/github/start', {
    headers: { Origin: 'https://unrelated.example' }, data: { bind: false },
  })).status()).toBe(403);
  const result = await request.post('/api/account/github/start', { data: {
    bind: false, redirect_uri: 'https://unrelated.example/github/callback',
    client_state: 'a'.repeat(64), proof_hash: 'b'.repeat(64),
  } });
  expect(result.status()).toBe(400);
  expect((await request.get('/api/web/session')).status()).toBe(401);
});
