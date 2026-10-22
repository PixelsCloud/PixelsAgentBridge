import { screenshotPath } from './support/isolatedFixture';
import { expect, test, type Page } from '@playwright/test';

const adminId = '10545b3a-e6f6-4b91-a122-9072055487ea';
const targetId = '20545b3a-e6f6-4b91-a122-9072055487ea';
const sessionId = '30545b3a-e6f6-4b91-a122-9072055487ea';
const usageUntil = Math.ceil(Date.now() / 3600000) * 3600000;
const usageFrom = usageUntil - 7 * 86400000;
type Operation = { id: string; target_id: string; kind: string; completed_at: number; status: string; propagation: string; sessions_revoked: number };

async function fixture(page: Page, options: { loseResponse?: boolean; githubOnly?: boolean; own?: boolean; language?: string; theme?: string } = {}) {
  const state = { writes: [] as { path: string; body: Record<string, unknown> }[], requests: [] as URL[], hidden: false, operations: new Map<string, Operation>(), sessions: [
    { id: sessionId, channel: 'browser', current: !!options.own, created_at: 1_000_000, last_active_at: 1_000_000 },
    { id: '40545b3a-e6f6-4b91-a122-9072055487ea', channel: 'unknown', current: false, created_at: 900_000, last_active_at: null },
  ], revision: 1 };
  await page.addInitScript(({ language, theme }) => { localStorage.setItem('pab-web-language', language); localStorage.setItem('pab-web-theme', theme); }, { language: options.language ?? 'en', theme: options.theme ?? 'light' });
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', ''); let data: unknown; let status = 200;
    const target = { id: targetId, username: 'Support user', status: 'active', server_admin: false, revision: state.revision, relay_limit_mbps: 10, created_at: 1_000_000, password_enabled: !options.githubOnly, github_enabled: !!options.githubOnly, device_count: 1, usage: { from: 0, until: 604800000, counters: { uploaded_bytes: '9007199254740993', connections: '7' }, updated_at: 1_000_000, incomplete: false } };
    target.usage.from = usageFrom; target.usage.until = usageUntil;
    const me = { id: options.own ? targetId : adminId, username: options.own ? 'Support user' : 'Administrator', server_admin: !options.own };
    if (path === '/config') data = { registration_enabled: true, version: 'fixture' };
    else if (path === '/session') data = me;
    else if (path === '/github') data = { enabled: false, password_enabled: true };
    else if (path === '/accounts') data = { items: [target], total: 1, page: 1, page_size: 20 };
    else if (path === `/accounts/${targetId}`) data = target;
    else if (path === '/traffic') data = { user_mbps: 10 };
    else if (path === '/devices') { state.requests.push(url); data = { items: [], total: 0, page: 1, page_size: 20 }; }
    else if (path === '/devices/diagnostics') data = { total: 0, reported: 0, not_reported: 0, systems: [], architectures: [], versions: [], versions_truncated: false };
    else if (path === '/usage') { state.requests.push(url); data = { counters: {}, points: [], updated_at: null }; }
    else if (path === '/usage/ranking') data = { items: [], total: 0, page: 1, page_size: 20 };
    else if (path.endsWith('/sessions') || path === '/sessions') data = { items: state.sessions, total: state.sessions.length, page: 1, page_size: 20 };
    else if (path.endsWith('/password-reset') || path.endsWith('/sessions/revoke')) {
      const body = route.request().postDataJSON(); state.writes.push({ path, body });
      const existing = state.operations.get(body.operation_id);
      const result = existing ?? { id: body.operation_id, target_id: targetId, kind: path.endsWith('/password-reset') ? 'password_reset' : 'session_revoke', completed_at: 1_000_000, status: 'completed', propagation: 'pending', sessions_revoked: body.scope === 'session' ? 1 : body.scope === 'others' ? state.sessions.filter(row => !row.current).length : state.sessions.length };
      if (!existing) {
        if (body.scope === 'others') state.sessions = state.sessions.filter(row => row.current);
        else if (body.scope === 'session') state.sessions = state.sessions.filter(row => row.id !== body.session_id);
        else if (body.scope === 'all' || body.revoke_sessions) state.sessions = [];
        if (result.kind === 'password_reset') state.revision++;
        state.operations.set(body.operation_id, result);
      }
      if (options.loseResponse && state.writes.length === 1) { state.hidden = true; await route.abort('connectionfailed'); return; }
      state.hidden = false; data = result;
    } else if (path.startsWith('/account-operations/')) {
      data = state.operations.get(path.split('/').at(-1)!);
      if (!data || state.hidden) { status = 404; data = { code: 'not_found' }; }
    } else throw new Error(`Unexpected account fixture request: ${path}`);
    await route.fulfill({ status, json: data });
  });
  return state;
}

async function openReset(page: Page) {
  await page.goto(`/accounts/${targetId}`);
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await dialog.getByLabel('New password', { exact: true }).fill('fixture replacement password');
  await dialog.getByLabel('Confirm password', { exact: true }).fill('fixture replacement password');
  return dialog;
}

test('account links preserve user and usage bounds across reload', async ({ page }) => {
  const state = await fixture(page); await page.goto('/accounts');
  await page.getByRole('button', { name: 'Support user', exact: true }).click();
  await expect(page).toHaveURL(`/accounts/${targetId}`);
  await expect(page.getByText('9007199254740993 B', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Associated devices', exact: true }).click();
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('user')).toBe(targetId);
  await page.reload(); await expect.poll(() => state.requests.at(-1)?.searchParams.get('user')).toBe(targetId);
  await page.goto(`/accounts/${targetId}`); await page.getByRole('button', { name: 'Account usage', exact: true }).click();
  await expect(page).toHaveURL(`/usage?user=${targetId}&period=7&from=${usageFrom}&until=${usageUntil}`);
  await expect.poll(() => state.requests.filter(url => url.pathname.endsWith('/usage')).some(url => url.searchParams.get('user') === targetId && url.searchParams.get('from') === String(usageFrom) && url.searchParams.get('until') === String(usageUntil))).toBe(true);
  await page.reload(); await expect(page).toHaveURL(`/usage?user=${targetId}&period=7&from=${usageFrom}&until=${usageUntil}`);
});

test('password reset defaults to revocation and removes password state after submission', async ({ page }) => {
  const state = await fixture(page); const dialog = await openReset(page);
  await expect(dialog.getByRole('checkbox')).toBeChecked();
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId('account-operation').first()).toContainText('Server operation committed');
  expect(state.writes[0].body.revoke_sessions).toBe(true);
  const storage = await page.evaluate(() => Object.fromEntries(Object.entries(localStorage).filter(([key]) => key.startsWith('pab-account-'))));
  expect(JSON.stringify(storage)).not.toContain('fixture replacement password');
  await page.getByRole('button', { name: 'Reset password', exact: true }).click();
  await expect(page.getByRole('dialog').getByLabel('New password', { exact: true })).toHaveValue('');
});

test('lost reset response restores original operation and retries without a new ID', async ({ page }) => {
  const state = await fixture(page, { loseResponse: true }); const dialog = await openReset(page);
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(page.getByTestId('account-operation').first()).toContainText('Operation outcome is unconfirmed');
  const id = state.writes[0].body.operation_id;
  await page.reload();
  await page.getByRole('button', { name: 'Retry original operation', exact: true }).click();
  const retry = page.getByRole('dialog');
  await expect(retry.getByLabel('New password', { exact: true })).toHaveValue('');
  await retry.getByLabel('New password', { exact: true }).fill('fixture replacement password');
  await retry.getByLabel('Confirm password', { exact: true }).fill('fixture replacement password');
  await retry.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(page.getByTestId('account-operation').first()).toContainText('Server operation committed');
  expect(state.writes).toHaveLength(2); expect(state.writes[1].body.operation_id).toBe(id); expect(state.operations.size).toBe(1);
});

test('GitHub-only account offers no local password reset', async ({ page }) => {
  const state = await fixture(page, { githubOnly: true }); await page.goto(`/accounts/${targetId}`);
  await expect(page.getByRole('button', { name: 'Reset password', exact: true })).toBeDisabled();
  await expect(page.getByText('This account has no local password. Recover access through GitHub.', { exact: true })).toBeVisible();
  expect(state.writes).toHaveLength(0);
});

test('own session controls keep current login and show server commit separately from propagation', async ({ page }) => {
  const state = await fixture(page, { own: true }); await page.goto('/account');
  await expect(page.getByText('Current sign-in', { exact: true })).toBeVisible();
  await expect(page.getByText('Unknown', { exact: true }).first()).toBeVisible();
  await page.getByRole('button', { name: 'Keep current, revoke other sign-ins', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(page.getByTestId('account-operation')).toContainText('Server operation committed');
  await expect(page.getByTestId('account-operation')).toContainText('Individual connection acknowledgement has not been confirmed.');
  expect(state.writes[0].body.scope).toBe('others'); expect(state.sessions).toHaveLength(1); expect(state.sessions[0].current).toBe(true);
});

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 390, 768, 1440]) {
  test(`account details layout ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await fixture(page, { language, theme }); await page.goto(`/accounts/${targetId}`);
    await expect(page.getByText('9007199254740993 B', { exact: true })).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 2);
    if (width === 1440) await page.screenshot({ path: screenshotPath(`account-${language}-${theme}.png`, '.build/admin-console/20261020-b3'), fullPage: true });
  });
}
