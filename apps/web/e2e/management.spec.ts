import { test, expect } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

// This suite targets only the named isolated database. No deployed credentials.
const root = resolve(import.meta.dirname, '../../..');
function sql(query: string) {
  return execFileSync('docker', ['exec', 'pab-web-test-20261003', 'psql', '-U', 'postgres', '-d', 'pab_account_test', '-At', '-v', 'ON_ERROR_STOP=1', '-c', query], { encoding: 'utf8', windowsHide: true }).trim();
}

async function fixture(name: string) {
  const executable = process.env.PAB_WEB_FIXTURE_BIN ?? resolve(root, `target/debug/examples/web-device-fixture${process.platform === 'win32' ? '.exe' : ''}`);
  const child = spawn(executable, ['wss://localhost:38443/control', resolve(root, '.build/web-test/cert.pem'), name], { windowsHide: true });
  const lines = createInterface({ input: child.stdout });
  const messages: Record<string, unknown>[] = []; let failure = '';
  lines.on('line', line => { try { messages.push(JSON.parse(line)); } catch { failure += line; } });
  child.stderr.on('data', value => { failure += value.toString(); });
  async function next(predicate: (value: Record<string, unknown>) => boolean) {
    await expect.poll(() => failure || messages.some(predicate), { timeout: 15000 }).toBe(true);
    const index = messages.findIndex(predicate); return messages.splice(index, 1)[0];
  }
  let ready: Record<string, unknown>;
  try { ready = await next(value => value.ready === true); } catch (e) { child.kill(); throw e; }
  return { child, ready, command: async (action: string, fields?: object) => { child.stdin.write(`${JSON.stringify({ action, ...fields })}\n`); await next(value => value.done === action); } };
}

test('administrator manages an unassigned device without claiming', async ({ page, request }) => {
  test.setTimeout(90000);
  const username = `owner-${Date.now()}`;
  const registration = await request.post('/api/web/register', { headers: { Origin: 'https://localhost:38443' }, data: { username, password: 'test password long enough' } });
  expect(registration.ok()).toBeTruthy();
  const me = await registration.json();
  expect(me.id).toMatch(/^[a-f0-9-]{36}$/);
  sql(`UPDATE users SET server_admin=true WHERE id='${me.id}'`);
  await page.context().addCookies((await request.storageState()).cookies);
  const device = await fixture(`web-fixture-${Date.now()}`);
  try {
    const code = String(device.ready.device_code);
    await page.goto('/all-devices?q=' + code + '&owner=unclaimed&page=2');
    await expect(page).toHaveURL('/devices?q=' + code);
    await expect(page.getByRole('menuitem', { name: 'Device list', exact: true })).toHaveCount(1);
    await expect(page.getByRole('menuitem', { name: 'My devices', exact: true })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'All devices', exact: true })).toHaveCount(0);
    await expect(page.getByRole('heading', { name: 'Device list', exact: true })).toBeVisible();
    await expect(page.getByRole('columnheader', { name: 'Owner', exact: true })).toHaveCount(0);
    await expect(page.getByRole('combobox', { name: 'Owner', exact: true })).toHaveCount(0);
    await expect(page.getByRole('menuitem', { name: 'Claim device', exact: true })).toHaveCount(0);
    expect((await request.post('/api/web/claims', { headers: { Origin: 'https://localhost:38443' }, data: { device_code: code, request_id: crypto.randomUUID() } })).status()).toBe(404);
    await expect(page.getByText('Online', { exact: true }).last()).toBeVisible();
    await device.command('second'); await device.command('drop_second');
    await expect(page.getByText('Online', { exact: true }).last()).toBeVisible();
    await device.command('disconnect');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Offline');
    await device.command('reconnect');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Online');
    await page.goto('/online?q=' + code);
    await expect(page.getByRole('heading', { name: 'Online devices', exact: true })).toBeVisible();
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Online');
    await device.command('disconnect');
    await expect(page.getByText('No data', { exact: true })).toBeVisible();
    await page.goto('/devices?q=' + code + '&status=offline');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Offline');
    await device.command('reconnect');
    await page.goto('/');
    await expect(page.getByText('Unassigned', { exact: true })).toHaveCount(0);
    expect((await (await request.get('/api/web/overview')).json()).unclaimed).toBeUndefined();
    await page.getByRole('button', { name: 'Device list', exact: true }).click();
    await expect(page).toHaveURL('/devices');
    await page.getByPlaceholder('Search name or device code').fill(code);
    await page.getByPlaceholder('Search name or device code').press('Enter');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Online');
    await page.locator('.device-link').click();
    await page.getByRole('button', { name: 'Rename', exact: true }).click();
    await page.getByLabel('Name', { exact: true }).fill('Renamed browser fixture');
    await page.mouse.click(5, 5);
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Renamed browser fixture', { exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Remove ownership', exact: true })).toHaveCount(0);
    const detail = await request.get(`/api/web/devices/${device.ready.device_id}`);
    expect(detail.status()).toBe(200);
    const info = await detail.json();
    expect(info.owner).toBeUndefined(); expect(info.owner_id).toBeUndefined();
    await expect(page.getByText('Owner', { exact: true })).toHaveCount(0);
    expect((await request.post(`/api/web/devices/${device.ready.device_id}/unbind`, { headers: { Origin: 'https://localhost:38443' }, data: { revision: info.revision } })).status()).toBe(404);
  } finally { device.child.kill(); }
});

test('administrator user bandwidth and all management pages', async ({ page, request }) => {
  test.setTimeout(90000);
  const username = `admin-${Date.now()}`;
  const response = await request.post('/api/web/register', { headers: { Origin: 'https://localhost:38443' }, data: { username, password: 'test password long enough' } });
  const me = await response.json(); expect(response.ok()).toBeTruthy();
  expect(me.id).toMatch(/^[a-f0-9-]{36}$/);
  sql(`UPDATE users SET server_admin=true WHERE id='${me.id}'`);
  await page.context().addCookies((await request.storageState()).cookies);
  await page.goto('/');
  for (const name of ['Device list', 'Accounts', 'Management changes', 'Relay']) {
    await page.getByRole('menuitem', { name, exact: true }).click();
    await expect(page.getByRole('heading', { name, exact: true })).toBeVisible();
    await expect(page.getByText('The service is unavailable. Please try again.', { exact: true })).toHaveCount(0);
  }
  await expect(page.getByRole('menuitem', { name: 'Teams', exact: true })).toHaveCount(0);
  await page.getByRole('menuitem', { name: 'Accounts', exact: true }).click();
  await page.getByRole('searchbox', { name: 'Accounts' }).fill(username);
  await page.getByRole('searchbox', { name: 'Accounts' }).press('Enter');
  const row = page.getByRole('row').filter({ hasText: username });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Bandwidth limits', exact: true }).click();
  await page.getByRole('dialog').getByRole('spinbutton').fill('30');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('30 Mbps', { exact: true })).toBeVisible();
  await row.getByRole('button', { name: 'Bandwidth limits', exact: true }).click();
  await page.getByRole('dialog').getByRole('spinbutton').fill('');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('30 Mbps', { exact: true })).toHaveCount(0);
  await page.screenshot({ path: resolve(root, '.build/web-test/admin-light.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Dark', exact: true }).click();
  await expect(page.locator('.ant-menu-item').first()).toHaveCSS('color', 'rgba(255, 255, 255, 0.65)');
  await page.screenshot({ path: resolve(root, '.build/web-test/admin-dark.png'), fullPage: true, animations: 'disabled' });
});

test('responsive languages, URL state, transient failure, reconnect and revoked session', async ({ page, request, context }) => {
  test.setTimeout(90000);
  const response=await request.post('/api/web/register',{headers:{Origin:'https://localhost:38443'},data:{username:`resilience-${Date.now()}`,password:'test password long enough'}});
  const me=await response.json();expect(response.ok()).toBeTruthy();
  await context.addCookies((await request.storageState()).cookies);
  const errors:string[]=[];page.on('pageerror',error=>errors.push(error.message));
  await page.goto('/devices?q=absent&page=2');
  await expect(page.getByPlaceholder('Search name or device code')).toHaveValue('absent');
  await expect(page.getByText('No data',{exact:true})).toBeVisible();
  for(const width of [1440,1024,768,390]){
    await page.setViewportSize({width,height:900});
    await expect.poll(()=>page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
    await page.screenshot({path:resolve(root,`.build/web-test/devices-${width}.png`),animations:'disabled'});
  }
  await page.setViewportSize({width:1280,height:900});
  for(const [language,heading] of [['繁體中文','裝置清單'],['简体中文','设备列表'],['English','Device list']]) {
    await page.locator('.console-header .ant-select').click();await page.locator('.ant-select-item-option-content').getByText(language,{exact:true}).click();
    await expect(page.getByRole('heading',{name:heading,exact:true})).toBeVisible();
  }
  await page.route('**/api/web/devices?**',route=>route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({code:'server_error'})}));
  await page.getByRole('button',{name:'Refresh',exact:true}).click();
  await expect(page.getByText('The service is unavailable. Please try again.',{exact:true})).toBeVisible();
  await page.unroute('**/api/web/devices?**');await page.getByRole('button',{name:'Refresh',exact:true}).click();
  await expect(page.getByText('The service is unavailable. Please try again.',{exact:true})).toHaveCount(0);
  await context.setOffline(true);
  await expect(page.getByText('Live updates interrupted. Reconnecting…',{exact:true})).toBeVisible({timeout:20000});
  await context.setOffline(false);
  await expect(page.getByText('Live updates interrupted. Reconnecting…',{exact:true})).toHaveCount(0,{timeout:20000});
  sql(`DELETE FROM web_sessions WHERE user_id='${me.id}'`);
  await expect(page.getByRole('heading',{name:'Sign in',exact:true})).toBeVisible({timeout:20000});
  expect(errors).toEqual([]);
});

test('100 simultaneous event connections, safe payloads and rejection outside session',async({page,request,browser})=>{
  test.setTimeout(60000);
  const response=await request.post('/api/web/register',{headers:{Origin:'https://localhost:38443'},data:{username:`subscriptions-${Date.now()}`,password:'test password long enough'}});expect(response.ok()).toBeTruthy();
  await page.context().addCookies((await request.storageState()).cookies);await page.goto('/');
  await expect(page.getByRole('heading',{name:'Overview',exact:true})).toBeVisible();
  const elapsed=await page.evaluate(async()=>{
    const started=performance.now();const sockets:WebSocket[]=[];
    try { await Promise.all(Array.from({length:100},()=>new Promise<void>((resolve,reject)=>{
      const socket=new WebSocket(`wss://${location.host}/api/web/events`);sockets.push(socket);
      const timer=setTimeout(()=>reject(new Error('WS snapshot timeout')),15000);
      socket.onerror=()=>{clearTimeout(timer);reject(new Error('WS failed'));};
      socket.onmessage=e=>{clearTimeout(timer);const value=JSON.parse(e.data);if(Object.keys(value).sort().join(',')!=='sequence,type'||value.type!=='refresh')reject(new Error('Unexpected resource payload'));else resolve();};
    })));return performance.now()-started; }finally {sockets.forEach(s=>s.close());}
  });
  console.log(`100 WebSocket connections received initial snapshots in ${Math.round(elapsed)} ms`);
  const other=await browser.newContext({ignoreHTTPSErrors:true});const otherPage=await other.newPage();
  try{
    await otherPage.goto('https://localhost:38443/');
    expect(await otherPage.evaluate(()=>new Promise<boolean>(resolve=>{const socket=new WebSocket(`wss://${location.host}/api/web/events`);socket.onopen=()=>{socket.close();resolve(false);};socket.onerror=()=>resolve(true);}))).toBe(true);
    await otherPage.goto('about:blank');
    expect(await otherPage.evaluate(()=>new Promise<boolean>(resolve=>{const socket=new WebSocket('wss://localhost:38443/api/web/events');socket.onopen=()=>{socket.close();resolve(false);};socket.onerror=()=>resolve(true);}))).toBe(true);
  }finally{await other.close();}
});
