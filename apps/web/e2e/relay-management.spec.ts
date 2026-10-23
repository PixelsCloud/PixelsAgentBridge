import { screenshotPath } from './support/isolatedFixture';
import { expect, test, type Page } from '@playwright/test';

type Operation = ReturnType<typeof operation>;
function operation(id: string) {
  return {
    id, node_id: 'relay-a', phase: 'waiting', action: 'maintenance', actor: 'Administrator',
    started_at_unix_ms: 1_000_000, deadline_unix_ms: 1_030_000, server_now_unix_ms: 1_000_000,
    completed_at_unix_ms: null as number | null, capacity_sampled_at_unix_ms: 1_000_000,
    remaining_capacity_mbps: 100 as number | null, remaining_host_bandwidth: false, error_code: null as string | null, allow_outage: false,
  };
}

test('managed host bandwidth nodes have normal actions and no deployment mode switch', async ({ page }) => {
  const state = await fixture(page, { telemetry: true });
  await page.goto('/relay');
  await expect(page.locator('.relay-service-card')).not.toContainText('Deployment mode');
  await expect(page.getByRole('button', { name: 'Maintain', exact: true })).toBeEnabled();
  await expect(page.locator('.ant-table-tbody')).toContainText('Full host bandwidth');
  await maintain(page);
  state.current!.remaining_host_bandwidth = true;
  state.current!.remaining_capacity_mbps = null;
  await expect(page.getByTestId('relay-operation')).toContainText('Full host bandwidth', { timeout: 10000 });
});

async function fixture(page: Page, options: { loseResponse?: boolean; noBackup?: boolean; language?: string; theme?: string; legacy?: boolean; failReload?: boolean; telemetry?: boolean } = {}) {
  const state = { current: undefined as Operation | undefined, writes: [] as { operation_id: string; allow_outage: boolean; state: string }[], requests: [] as URL[], operationRequests: [] as string[], hideOperation: false, failReads: false, reloads: 0, telemetryCurrent: true, nodeState: 'active', resumeGate: undefined as Promise<void> | undefined, failResume: false };
  await page.addInitScript(({ language, theme }) => {
    if (!localStorage.getItem('pab-web-language')) localStorage.setItem('pab-web-language', language);
    localStorage.setItem('pab-web-theme', theme);
  }, { language: options.language ?? 'en', theme: options.theme ?? 'light' });
  await page.routeWebSocket('**/api/web/events', () => {});
  await page.route('**/api/web/**', async route => {
    const url = new URL(route.request().url()); const path = url.pathname.replace('/api/web', '');
    let data: unknown; let status = 200;
    const summary = { configured: 2, available: state.nodeState === 'active' ? 2 : 1, maintenance: state.nodeState === 'maintenance' ? 1 : 0, disabled: state.nodeState === 'disabled' ? 1 : 0, abnormal: 0, history: 0 };
    const snapshot = { summary, mode: 'cluster', offline_seconds: 15, semantics_version: 1, sampled_at: 1_000_000, health: { server: 'responding', database: 'reachable', source: 'management_database_query', checked_at: 1_000_000, latency_grade: 'normal' } };
    if (path === '/config') data = { registration_enabled: true, version: 'fixture' };
    else if (path === '/session') data = { id: 'relay-test-admin', username: 'Administrator', server_admin: true };
    else if (path === '/overview') data = { total: 0, online: 0, offline: 0, accounts: 1, relays: 2, relay_status: snapshot };
    else if (path === '/traffic') data = { user_mbps: 10 };
    else if (path === '/service') data = { registration_enabled: true, policy_revision: 42 };
    else if (path === '/relays') {
      state.requests.push(url);
      if (state.failReads) { data = { code: 'server_error' }; status = 500; }
      else data = { ...snapshot, mode: options.legacy ? 'legacy' : 'cluster', items: [{ id: 'relay-a', online: true, health: 'healthy', configured: true, admin_state: options.legacy ? null : state.nodeState, agent_version: 'fixture', last_seen: 1_000_000, applied_policy_version: 42, offered_policy_version: 42, capacity_mbps: options.legacy ? null : 100, operation_id: state.current?.id, boot_id: 'fixture-boot', heartbeat_age_ms: 3000, sample_current_instance: true, uploaded_bytes_exact: '9007199254740993', downloaded_bytes_exact: '123', ...(options.telemetry ? { url: 'https://relay.example/', capacity_mode: 'host', connections_exact: '0', uploaded_bytes_exact: '0', downloaded_bytes_exact: '1536', telemetry_sampled_at: 1_000_000, telemetry_current: state.telemetryCurrent } : {}) }], total: 1, page: 1, page_size: Number(url.searchParams.get('page_size') ?? 20) };
    } else if (path === '/relays/reload') {
      expect(route.request().method()).toBe('POST'); state.reloads++;
      status = options.failReload ? 500 : 200; data = options.failReload ? { code: 'server_error' } : { reloaded: true };
    } else if (path === '/relays/relay-a/state') {
      const input = route.request().postDataJSON(); state.writes.push(input);
      if (input.state === 'active' && state.failResume) { await route.fulfill({ status: 409, json: { code: 'relay_not_ready' } }); return; }
      if (input.state === 'active' && state.resumeGate) await state.resumeGate;
      if (options.noBackup && !input.allow_outage) { data = { code: 'relay_no_backup' }; status = 409; }
      else {
        if (state.current?.id !== input.operation_id) state.current = { ...operation(input.operation_id), action: input.state === 'active' ? 'activate' : input.state === 'disabled' ? 'disable' : 'maintenance', phase: input.state === 'maintenance' ? 'waiting' : 'completed' };
        state.nodeState = input.state;
        if (options.loseResponse && state.writes.length === 1) { state.hideOperation = true; await route.abort('connectionfailed'); return; }
        state.hideOperation = false; data = state.current;
      }
    } else if (path.startsWith('/relay-operations/')) {
      state.operationRequests.push(path);
      if (state.failReads) { data = { code: 'server_error' }; status = 500; }
      else if (!state.current || path !== `/relay-operations/${state.current.id}` || state.hideOperation) { data = { code: 'not_found' }; status = 404; }
      else data = state.current;
    } else throw new Error(`Unexpected fixture request: ${path}`);
    await route.fulfill({ status, json: data });
  });
  return state;
}

async function maintain(page: Page) {
  await page.goto('/relay');
  await page.getByRole('button', { name: 'Maintain', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click();
}

async function expectPaintedDialog(page: Page) {
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect.poll(() => dialog.evaluate(element => {
    // Playwright visibility deliberately accepts opacity 0. Check the entire
    // painted ancestor chain and hit target to catch invisible modal blockers.
    for (let node: Element | null = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (Number(style.opacity) < .99 || style.visibility !== 'visible' || style.display === 'none') return false;
    }
    const rect = element.getBoundingClientRect();
    return element.contains(document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2));
  })).toBe(true);
}

for (const reducedMotion of ['reduce', 'no-preference'] as const) for (const width of [390, 1440]) for (const language of ['en', 'zh-CN']) {
  test(`maintenance confirmation paints, cancels and submits ${reducedMotion} ${width} ${language}`, async ({ page }) => {
    await page.emulateMedia({ reducedMotion }); await page.setViewportSize({ width, height: 1000 });
    const state = await fixture(page, { language });
    const labels = language === 'en' ? ['Maintain', /Cancel/, 'Confirm', 'Resume service'] : ['开始维护', /取\s*消/, '确认', '恢复服务'];
    await page.goto('/relay');
    const open = () => page.getByRole('button', { name: labels[0], exact: true }).click();
    await open(); await expectPaintedDialog(page);
    expect(state.writes).toHaveLength(0);
    await page.getByRole('dialog').getByRole('button', { name: labels[1] }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    expect(state.writes).toHaveLength(0);
    await open(); await expectPaintedDialog(page);
    await page.getByRole('dialog').getByRole('button', { name: labels[2], exact: true }).click();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByTestId('relay-operation')).toBeVisible();
    await page.getByRole('button', { name: labels[3], exact: true }).click();
    await expect(page.getByRole('button', { name: labels[0], exact: true })).toBeEnabled();
    expect(state.writes.map(row => row.state)).toEqual(['maintenance', 'active']);
    expect(state.nodeState).toBe('active');
  });
}

test('runtime reduced motion changes keep confirmation and detail dialogs painted and dismissible', async ({ page }) => {
  await fixture(page); await page.emulateMedia({ reducedMotion: 'no-preference' }); await page.goto('/relay');
  await page.getByRole('button', { name: 'Maintain', exact: true }).click(); await expectPaintedDialog(page);
  await page.emulateMedia({ reducedMotion: 'reduce' }); await expectPaintedDialog(page);
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'Reload node configuration', exact: true }).click(); await expectPaintedDialog(page);
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.getByRole('button', { name: 'Details', exact: true }).click(); await expectPaintedDialog(page);
  await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await page.getByRole('button', { name: 'Maintain', exact: true }).click(); await expectPaintedDialog(page);
});

test('resume service submits immediately and shows progress before updating the node', async ({ page }) => {
  const state = await fixture(page); await maintain(page);
  const original = state.current!.id;
  let release!: () => void; state.resumeGate = new Promise<void>(resolve => { release = resolve; });
  await page.getByRole('button', { name: 'Resume service', exact: true }).click();
  try {
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Resuming…', exact: true })).toBeDisabled();
    await expect.poll(() => state.writes.length).toBe(2);
    expect(state.writes[1].state).toBe('active'); expect(state.writes[1].allow_outage).toBe(false);
    expect(state.writes[1].operation_id).not.toBe(original);
    expect(state.operationRequests).not.toContain(`/relay-operations/${state.writes[1].operation_id}`);
    await expect(page.getByTestId('relay-operation')).not.toContainText('Operation outcome is unconfirmed.');
  } finally { release(); }
  await expect(page.getByRole('button', { name: 'Maintain', exact: true })).toBeEnabled();
  await expect(page.getByRole('button', { name: 'Resume service', exact: true })).toHaveCount(0);
  await expect(page.getByTestId('relay-operation')).toContainText('Administrative action completed', { timeout: 2000 });
  expect(state.writes).toHaveLength(2);
});

test('resume service displays rejection outside a dialog and can retry', async ({ page }) => {
  const state = await fixture(page); await maintain(page); state.failResume = true;
  await page.getByRole('button', { name: 'Resume service', exact: true }).click();
  await expect(page.locator('.ant-alert').filter({ hasText: 'The node is not ready.' })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Resume service', exact: true })).toBeEnabled();
  state.failResume = false;
  await page.getByRole('button', { name: 'Resume service', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Maintain', exact: true })).toBeEnabled();
  expect(state.writes.map(row => row.state)).toEqual(['maintenance', 'active', 'active']);
});

test('maintenance tracks server phases and restores the operation after reload', async ({ page }) => {
  const state = await fixture(page); await maintain(page);
  const panel = page.getByTestId('relay-operation');
  await expect(panel.getByText('Waiting for transition', { exact: true })).toBeVisible();
  await expect(panel).toContainText('100 Mbps');
  await expect(panel).toContainText('30 s');
  await expect(panel).not.toContainText('Ready means the maintenance wait has ended.');
  const id = state.current!.id;
  await page.reload();
  await expect(panel).toContainText(id);
  await expect(panel.getByText('Waiting for transition', { exact: true })).toBeVisible();
  state.current!.phase = 'ready';
  await expect(panel.getByText('Ready for operations', { exact: true })).toBeVisible({ timeout: 10000 });
  await expect(panel).toContainText('Ready means the maintenance wait has ended. It does not confirm an upgrade or drained sessions.');
  expect(state.writes).toHaveLength(1);
  await expect(panel).toContainText('End-to-end forwarding probe: not run by this page.');
});

test('closing and reopening the page restores the same operation without another write', async ({ page }) => {
  const state = await fixture(page); await maintain(page);
  await expect(page.getByTestId('relay-operation')).toContainText('Waiting for transition');
  const id = state.current!.id;
  const context = page.context();
  await page.close();
  const reopened = await context.newPage();
  const restored = await fixture(reopened);
  restored.current = state.current;
  await reopened.goto('/relay');
  await expect(reopened.getByTestId('relay-operation')).toContainText(id);
  await expect(reopened.getByTestId('relay-operation')).toContainText('Waiting for transition');
  expect(state.writes).toHaveLength(1);
  expect(restored.writes).toHaveLength(0);
});

test('lost responses retain the original request across reload and block new actions', async ({ page }) => {
  const state = await fixture(page, { loseResponse: true }); await maintain(page);
  const panel = page.getByTestId('relay-operation');
  await expect(panel).toContainText('Operation outcome is unconfirmed.');
  await expect(page.getByRole('button', { name: 'Disable', exact: true })).toBeDisabled();
  const id = state.writes[0].operation_id;
  await page.reload();
  await expect(panel).toContainText(id);
  await panel.getByRole('button', { name: 'Retry original request', exact: true }).click();
  await expect(panel.getByText('Waiting for transition', { exact: true })).toBeVisible({ timeout: 10000 });
  expect(state.writes).toHaveLength(2);
  expect(state.writes[1]).toEqual(state.writes[0]);
});

test('no backup requires explicit outage consent before creating an operation', async ({ page }) => {
  const state = await fixture(page, { noBackup: true }); await maintain(page);
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('alert')).toContainText('No healthy backup is available.');
  expect(state.current).toBeUndefined();
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(page.getByTestId('relay-operation')).toContainText('Waiting for transition');
  expect(state.writes.map(item => item.allow_outage)).toEqual([false, true]);
});

test('expired operation records explain their history and preserve unknown capacity', async ({ page }) => {
  const state = await fixture(page); await maintain(page);
  state.current!.phase = 'failed'; state.current!.error_code = 'operator_lease_expired'; state.current!.remaining_capacity_mbps = null;
  await page.reload();
  const panel = page.getByTestId('relay-operation');
  await expect(panel).toContainText('Record expired');
  await expect(panel).toContainText('Viewing it does not start maintenance again.');
  await expect(panel.getByText('Operation failed', { exact: true })).toHaveCount(0);
  await expect(panel).toContainText('Unknown');
  await expect(page.locator('.ant-table-tbody').getByText('Maintenance', { exact: true })).toBeVisible();
});

test('Relay classification filters preserve the same status across reload', async ({ page }) => {
  const state = await fixture(page);
  await page.goto('/relay');
  await page.getByRole('button', { name: 'Accepting connections: 2', exact: true }).click();
  await expect(page).toHaveURL('/relay?status=available');
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('status')).toBe('available');
  await page.reload();
  await expect.poll(() => state.requests.at(-1)?.searchParams.get('status')).toBe('available');
});

test('node details preserve exact counters and request failures replace healthy status with unknown', async ({ page }) => {
  const state = await fixture(page); await maintain(page);
  await expect(page.getByTestId('relay-operation')).toContainText('Waiting for transition');
  await page.getByRole('button', { name: 'Details', exact: true }).click();
  await expect(page.locator('.relay-detail')).toContainText('8 PiB');
  await expect(page.locator('.relay-detail span[title="9007199254740993 B"]')).toBeVisible();
  await expect(page.locator('.relay-detail')).toContainText('fixture-boot');
  await page.locator('.relay-detail').getByRole('button', { name: 'Close', exact: true }).click();
  state.failReads = true;
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.locator('.ant-table-tbody').getByText('Healthy', { exact: true })).toHaveCount(0, { timeout: 10000 });
  await expect(page.locator('.ant-table-tbody').getByText('Unknown', { exact: true }).first()).toBeVisible();
  await expect(page.getByTestId('relay-operation')).toContainText('Operation outcome is unconfirmed.', { timeout: 10000 });
  await expect(page.getByRole('button', { name: 'Resume service', exact: true })).toBeDisabled();
});

test('legacy nodes expose service information and details without blank cluster columns or bandwidth settings', async ({ page }) => {
  const state = await fixture(page, { legacy: true }); await page.goto('/relay');
  await expect(page.getByRole('menuitem', { name: 'Relay nodes', exact: true })).toBeVisible();
  await expect(page.locator('.relay-service-card')).not.toContainText('Legacy mode');
  await expect(page.locator('.relay-service-card')).toContainText('Read succeeded');
  await expect(page.getByText('Bandwidth limits', { exact: true })).toHaveCount(0);
  await expect(page.getByRole('columnheader', { name: 'Node capacity', exact: true })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Maintain', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Details', exact: true }).click();
  await expect(page.locator('.relay-detail')).toContainText('42');
  await expect(page.locator('.relay-detail')).toContainText('No runtime report has arrived');
  expect(state.writes).toHaveLength(0);
});

test('configuration reload is available in legacy mode and only submits after confirmation', async ({ page }) => {
  const state = await fixture(page, { legacy: true }); await page.goto('/relay');
  await page.getByRole('button', { name: 'Reload node configuration', exact: true }).click();
  const dialog = page.getByRole('dialog'); await expect(dialog).toContainText('does not restart services');
  expect(state.reloads).toBe(0); await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect(state.reloads).toBe(0);
  await page.getByRole('button', { name: 'Reload node configuration', exact: true }).click();
  await dialog.getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect.poll(() => state.reloads).toBe(1); expect(state.writes).toHaveLength(0);
});

test('failed configuration reload reports an error without claiming success', async ({ page }) => {
  const state = await fixture(page, { legacy: true, failReload: true }); await page.goto('/relay');
  await page.getByRole('button', { name: 'Reload node configuration', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Confirm', exact: true }).click();
  await expect(page.locator('.ant-message-error')).toBeVisible(); await expect(page.locator('.ant-message-success')).toHaveCount(0);
  expect(state.reloads).toBe(1);
});

test('retired Traditional Chinese preferences migrate to Simplified Chinese in both language controls', async ({ page }) => {
  await fixture(page, { language: 'zh-TW', legacy: true }); await page.goto('/relay');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  expect(await page.evaluate(() => localStorage.getItem('pab-web-language'))).toBe('zh-CN');
  const language = page.locator('.console-header select.language-select');
  await expect(language.locator('option')).toHaveCount(2); await expect(language).toHaveValue('zh-CN');
  await language.selectOption('en'); await page.goto('/settings');
  await expect(page.locator('.preferences-grid select option')).toHaveCount(2);
  await expect(page.locator('.preferences-grid select')).toHaveValue('en');
});

test('single-node runtime reports show host bandwidth, zero connections and formatted process traffic', async ({ page }) => {
  await fixture(page, { legacy: true, telemetry: true }); await page.goto('/relay');
  await expect(page.getByRole('columnheader', { name: 'Node capacity', exact: true })).toBeVisible();
  await expect(page.locator('.ant-table-tbody')).toContainText('Full host bandwidth');
  await page.getByRole('button', { name: 'Details', exact: true }).click();
  const detail = page.locator('.relay-detail');
  await expect(detail).toContainText('https://relay.example/');
  await expect(detail).toContainText('1.5 KiB');
  await expect(detail).toContainText('0 B');
  await expect(detail).toContainText('Runtime report time');
  await expect(detail).not.toContainText('No runtime report has arrived');
  await page.setViewportSize({ width: 360, height: 900 });
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(362);
});

test('expired process telemetry stays unknown even while policy connectivity is healthy', async ({ page }) => {
  const state = await fixture(page, { legacy: true, telemetry: true }); state.telemetryCurrent = false;
  await page.goto('/relay');
  await expect(page.locator('.ant-table-tbody')).toContainText('Healthy');
  await page.getByRole('button', { name: 'Details', exact: true }).click();
  const detail = page.locator('.relay-detail');
  await expect(detail).toContainText('Full host bandwidth');
  await expect(detail).not.toContainText('1.5 KiB');
  await expect(detail).not.toContainText('0 B');
  await expect(detail).toContainText('Unknown');
});

for (const language of ['en', 'zh-CN']) for (const theme of ['light', 'dark']) for (const width of [360, 390, 768, 1440]) {
  test(`operation layout ${language} ${theme} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    const state = await fixture(page, { language, theme });
    const id = '70545b3a-e6f6-4b91-a122-9072055487ea';
    state.current = operation(id);
    await page.addInitScript(value => localStorage.setItem('pab-relay-operation:relay-test-admin', JSON.stringify({ id: value, unconfirmed: false })), id);
    await page.goto('/relay');
    const panel = page.getByTestId('relay-operation');
    await expect(panel).toContainText(language === 'en' ? 'Waiting for transition' : language === 'zh-CN' ? '等待过渡' : '等待過渡');
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await expect(panel).toContainText('100 Mbps');
    // Horizontal table scrolling is contained; the document must fit the viewport.
    const documentWidth = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(documentWidth).toBeLessThanOrEqual(width + 2);
    if (width === 1440) await page.screenshot({ path: screenshotPath(`relay-${language}-${theme}.png`, '.build/admin-console/20261020'), fullPage: true });
    expect(state.writes).toHaveLength(0);
  });
}


test('node bandwidth can be edited and DNS IPs identify each Relay', async ({ page }) => {
  await fixture(page, { telemetry: true });
  let capacity = 0;
  const writes: unknown[] = [];
  await page.route('**/api/web/relays?*', route => route.fulfill({ json: { mode:'cluster', items:[{id:'relay-a',url:'https://relay.example/',ip_addresses:['192.0.2.10','2001:db8::10'],online:true,health:'healthy',admin_state:'active',configured:true,capacity_mbps:capacity,capacity_mode:capacity ? 'configured' : 'host',sample_current_instance:true}],total:1,page:1,page_size:20 } }));
  await page.route('**/api/web/relays/relay-a/capacity', async route => {
    const body = route.request().postDataJSON(); writes.push(body); capacity = body.capacity_mbps;
    await route.fulfill({json:{capacity_mbps:capacity}});
  });
  await page.goto('/relay');
  await expect(page.locator('.ant-table-tbody')).toContainText('192.0.2.10, 2001:db8::10');
  await page.getByRole('button',{name:'Set node bandwidth relay-a',exact:true}).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('spinbutton')).toHaveValue('200');
  await dialog.getByRole('spinbutton').fill('300');
  await dialog.getByRole('textbox',{name:'Machine IP',exact:true}).fill('192.0.2.10');
  await dialog.getByRole('button',{name:'Save',exact:true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.locator('.ant-table-tbody')).toContainText('300 Mbps');
  expect(writes).toEqual([{capacity_mbps:300,machine_ip:'192.0.2.10'}]);
  await page.getByRole('button',{name:'Set node bandwidth relay-a',exact:true}).click();
  await expect(page.getByRole('dialog').getByRole('spinbutton')).toHaveValue('300');
  await page.getByRole('dialog').getByRole('button',{name:'Cancel',exact:true}).click();
  await page.getByRole('button',{name:'Details',exact:true}).click();
  await expect(page.getByRole('dialog')).toContainText('2001:db8::10');
});

test('failed bandwidth saves remain editable and show the error', async ({ page }) => {
  await fixture(page, { telemetry: true });
  await page.route('**/api/web/relays/relay-a/capacity', route => route.fulfill({status:500,json:{code:'server_error'}}));
  await page.goto('/relay');
  await page.getByRole('button',{name:'Set node bandwidth relay-a',exact:true}).click();
  await page.getByRole('dialog').getByRole('button',{name:'Save',exact:true}).click();
  await expect(page.getByRole('dialog').getByRole('alert')).toBeVisible();
  await expect(page.getByRole('dialog').getByRole('spinbutton')).toBeEnabled();
});


test('disabled nodes keep real counts until confirmed and hide stale counts when offline', async ({ page }) => {
  await fixture(page, { telemetry: true });
  let applied=false;let online=true;
  await page.route('**/api/web/relays?*',route=>route.fulfill({json:{mode:'cluster',items:[{id:'relay-a',admin_state:'disabled',operation_id:'00000000-0000-4000-8000-000000000001',state_applied:applied,state_drained:applied,node_control_version:1,online,health:online?'healthy':'offline',configured:true,capacity_mbps:200,capacity_mode:'configured',connections_exact:applied?'0':'6',telemetry_current:online,sample_current_instance:true}],total:1,page:1,page_size:20}}));
  await page.goto('/relay');const row=page.locator('.ant-table-tbody>.ant-table-row');
  await expect(row).toContainText('Disable pending confirmation');await expect(row.locator('td').nth(5)).toHaveText('6');
  applied=true;await page.getByRole('button',{name:'Refresh',exact:true}).click();await expect(row).not.toContainText('Disable pending confirmation');await expect(row.locator('td').nth(5)).toHaveText('0');
  online=false;applied=false;await page.getByRole('button',{name:'Refresh',exact:true}).click();await expect(row.locator('td').nth(5)).toHaveText('Unknown');
});
