// Only attaches to an explicitly started, isolated debug Desktop.
import {createRequire} from 'node:module';
const require = createRequire(new URL('../../apps/desktop/package.json', import.meta.url));
const {chromium, expect} = require('@playwright/test');
const [port, phase] = process.argv.slice(2);
if (!/^\d+$/.test(port)) throw Error('Expected loopback debugger port');
const browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
try {
  let page;
  for (let n = 0; n < 200 && !page; n++) {
    page = browser.contexts().flatMap(c => c.pages()).find(p => p.url().includes('tauri.localhost'));
    if (!page) await new Promise(r => setTimeout(r, 100));
  }
  if (!page) throw Error('Isolated main window missing');
  await expect.poll(() => page.evaluate(async () => {
    try { return (await window.__TAURI_INTERNALS__.invoke('device_status')).control_phase; }
    catch { return ''; }
  }), {timeout: 20000}).toBe('Unconfigured');
  const status = await page.evaluate(() => window.__TAURI_INTERNALS__.invoke('device_status'));
  if (status.device_id || status.device_code || status.temporary_password || !status.executor_running) {
    throw Error('Unconfigured service exposed device credentials or stopped');
  }
  await expect(page.locator('.home-credentials')).toHaveCount(0);
  await expect(page.locator('.home-device .empty-device')).toBeVisible();
  const homeMessage = await page.locator('.home-device .empty-device').textContent();
  await page.getByRole('menuitem', {name: /^设置$|^設定$|^Settings$/}).click();
  await page.getByRole('menuitem', {name: /^网络连接$|^網路連線$|^Network connection$/}).click();
  const control = page.locator('#server-control'), relay = page.locator('#server-relay');
  await expect(control).toBeEnabled();
  await expect(relay).toBeEnabled();
  const customer = {controlUrl: 'wss://127.0.0.1:39491/control', relayUrl: 'https://127.0.0.1:39492'};
  const expected = phase === 'customer' ? customer : {controlUrl: '', relayUrl: ''};
  await expect(control).toHaveValue(expected.controlUrl);
  await expect(relay).toHaveValue(expected.relayUrl);
  if (phase === 'empty') {
    await control.fill(customer.controlUrl);
    await relay.fill(customer.relayUrl);
  } else if (phase === 'customer') {
    await control.fill('');
    await relay.fill('');
  } else if (phase !== 'cleared') throw Error('Unknown phase');
  if (phase !== 'cleared') {
    await page.getByRole('button', {name: /^保存服务器设置$|^儲存伺服器設定$|^Save server settings$/}).click();
    await expect(page.getByRole('button', {name: /重启|重新啟動|Restart/})).toBeVisible({timeout: 15000});
    await expect(page.getByRole('alert')).toHaveCount(0);
  }
  console.log(JSON.stringify({phase, unconfigured: true, no_device_credentials: true,
    settings_persisted: true, home_message: homeMessage}));
} finally { await browser.close(); }
