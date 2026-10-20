// Uses only the loopback debugger of an explicitly started acceptance Desktop.
import {createRequire} from 'node:module';
const require = createRequire(new URL('../../apps/desktop/package.json', import.meta.url));
const {chromium, expect} = require('@playwright/test');
const [action, token, id] = process.argv.slice(2);
const browser = await chromium.connectOverCDP('http://127.0.0.1:39100');
try {
  const pages = () => browser.contexts().flatMap(c => c.pages());
  let main;
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    main = pages().find(p => p.url().includes('tauri.localhost') && !p.url().includes('desktopViewer='));
    if (main) break;
    await new Promise(r => setTimeout(r, 100));
  }
  if (!main) throw Error('Acceptance main window missing');
  const dialog = main.getByRole('dialog').filter({hasText: token});
  let result;
  if (action === 'show' || action === 'open') {
    await expect(dialog).toBeVisible({timeout: 45000});
    await expect(dialog).toHaveCSS('opacity', '1');
    if (action === 'open') {
      await dialog.getByRole('button', {name: /Open remote desktop|打开远程桌面|打開遠端桌面/}).click();
    }
    result = {observed: true};
  } else if (action === 'gone') {
    await expect(dialog).toHaveCount(0, {timeout: 12000});
    result = {observed: true};
  } else if (action === 'no-viewer') {
    if (pages().some(p => p.url().includes('desktopViewer='))) throw Error('Viewer opened automatically');
    result = {viewer_count: 0};
  } else {
    const viewers = pages().filter(p => p.url().includes('desktopViewer='));
    if (viewers.length !== 1) throw Error('Expected exactly one acceptance viewer');
    const viewer = viewers[0];
    const complete = viewer.getByRole('button', {name: /^Complete assistance|^完成协助/});
    if (action === 'ready') {
      await expect(viewer.getByRole('button', {name: /^Enable control$|^启用操作$/})).toBeEnabled({timeout: 35000});
      await expect(complete).toBeEnabled({timeout: 12000});
    } else if (action === 'unbound') {
      await expect(complete).toHaveCount(0, {timeout: 12000});
    } else if (action === 'stale-complete') {
      result = await viewer.evaluate(async id => {
        try {
          await window.__TAURI_INTERNALS__.invoke('desktop_viewer_assistance_complete', {id});
          return {accepted: true};
        } catch { return {accepted: false}; }
      }, id);
      if (result.accepted) throw Error('Terminated assistance was completed');
    } else if (action !== 'snapshot') throw Error('Unknown acceptance action');
    if (!result) result = await viewer.evaluate(async () => ({
      label: window.__TAURI_INTERNALS__.metadata.currentWindow.label,
      status: await window.__TAURI_INTERNALS__.invoke('desktop_viewer_start'),
    }));
  }
  console.log(JSON.stringify(result));
} catch (e) {
  console.error(String(e));
  process.exitCode = 1;
} finally {
  // Disconnect CDP; never close the app's browser context.
  await browser.close();
}
