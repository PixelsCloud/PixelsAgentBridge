import { test, expect } from '@playwright/test';
import { execFileSync, spawn } from 'node:child_process';
import { resolve } from 'node:path';
import { createInterface } from 'node:readline';

// This suite targets only the named isolated database. No deployed credentials.
const root = resolve(import.meta.dirname, '../../..');
function sql(query: string) {
  return execFileSync('docker', ['exec', 'pab-web-test-20261003', 'psql', '-U', 'postgres', '-d', 'pab_web_test', '-At', '-v', 'ON_ERROR_STOP=1', '-c', query], { encoding: 'utf8', windowsHide: true }).trim();
}

async function fixture(name: string) {
  const deployment = sql('SELECT id FROM deployments');
  const executable = process.env.PAB_WEB_FIXTURE_BIN ?? resolve(root, `target/debug/examples/web-device-fixture${process.platform === 'win32' ? '.exe' : ''}`);
  const child = spawn(executable, ['wss://localhost:38443/control', resolve(root, '.build/web-test/cert.pem'), deployment, name], { windowsHide: true });
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

test('real device presence, multi-connection, claim, rename and unbind', async ({ page, request }) => {
  test.setTimeout(90000);
  const username = `owner-${Date.now()}`;
  const registration = await request.post('/api/web/register', { headers: { Origin: 'https://localhost:38443' }, data: { username, password: 'test password long enough' } });
  expect(registration.ok()).toBeTruthy();
  await page.context().addCookies((await request.storageState()).cookies);
  const device = await fixture(`web-fixture-${Date.now()}`);
  try {
    const code = String(device.ready.device_code);
    await page.goto('/claims');
    await page.getByLabel('Device code', { exact: true }).fill(code);
    await page.getByRole('button', { name: 'Claim device', exact: true }).click();
    await expect(page.getByText('Awaiting device approval', { exact: true })).toBeVisible();
    const claims = await (await request.get('/api/web/claims')).json();
    await device.command('approve', { claim_id: claims.items[0].id });
    await expect(page.getByText('Approved', { exact: true })).toBeVisible();
    await page.getByRole('menuitem', { name: 'My devices', exact: true }).click();
    await expect(page.getByText('Online', { exact: true }).last()).toBeVisible();
    await device.command('second'); await device.command('drop_second');
    await expect(page.getByText('Online', { exact: true }).last()).toBeVisible();
    await device.command('disconnect');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Offline');
    await device.command('reconnect');
    await expect(page.locator('.ant-table-tbody .ant-badge-status-text')).toHaveText('Online');
    await page.locator('.device-link').click();
    await page.getByRole('button', { name: 'Rename', exact: true }).click();
    await page.getByLabel('Name', { exact: true }).fill('Renamed browser fixture');
    await page.mouse.click(5, 5);
    await expect(page.getByRole('dialog')).toBeVisible();
    await page.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Renamed browser fixture', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Remove ownership', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm', exact: true }).click();
    await expect(page.getByText('No data', { exact: true })).toBeVisible();
    expect((await request.get(`/api/web/devices/${device.ready.device_id}`)).status()).toBe(404);
  } finally { device.child.kill(); }
});

test('administrator team workflow and all management pages', async ({ page, request }) => {
  test.setTimeout(90000);
  const username = `admin-${Date.now()}`;
  const response = await request.post('/api/web/register', { headers: { Origin: 'https://localhost:38443' }, data: { username, password: 'test password long enough' } });
  const me = await response.json(); expect(response.ok()).toBeTruthy();
  expect(me.id).toMatch(/^[a-f0-9-]{36}$/);
  sql(`UPDATE users SET server_admin=true WHERE id='${me.id}'`);
  await page.context().addCookies((await request.storageState()).cookies);
  await page.goto('/');
  for (const name of ['All devices', 'Accounts', 'Management changes', 'Relay']) {
    await page.getByRole('menuitem', { name, exact: true }).click();
    await expect(page.getByRole('heading', { name, exact: true })).toBeVisible();
    await expect(page.getByText('The service is unavailable. Please try again.', { exact: true })).toHaveCount(0);
  }
  await page.getByRole('menuitem', { name: 'Teams', exact: true }).click();
  await page.getByRole('button', { name: 'Create team', exact: true }).click();
  const name = `Browser Team ${Date.now()}`;
  await page.getByLabel('Team name', { exact: true }).fill(name);
  await page.getByLabel('Owner account', { exact: true }).fill(username);
  await page.getByText(username, { exact: true }).last().click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const row = page.getByRole('row').filter({ hasText: name });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Bandwidth limits', exact: true }).click();
  await page.getByLabel('Total bandwidth (Mbps)', { exact: true }).fill('30');
  await page.getByLabel('Member bandwidth (Mbps)', { exact: true }).fill('6');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(row.getByText('30 Mbps', { exact: true })).toBeVisible();
  await row.getByRole('button', { name: 'Members', exact: true }).click();
  await expect(page.locator('.ant-table-tbody').getByText(username, { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Remove member', exact: true })).toBeDisabled();
  await page.screenshot({ path: resolve(root, '.build/web-test/admin-light.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Dark', exact: true }).click();
  await expect(page.locator('.ant-menu-item').first()).toHaveCSS('color', 'rgba(255, 255, 255, 0.65)');
  await expect(page.getByRole('button', { name: 'Add member', exact: true })).toHaveCSS('color', 'rgb(16, 39, 40)');
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
  for(const [language,heading] of [['繁體中文','我的裝置'],['简体中文','我的设备'],['English','My devices']]) {
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
  sql(`UPDATE users SET auth_revision=auth_revision+1 WHERE id='${me.id}'`);
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
