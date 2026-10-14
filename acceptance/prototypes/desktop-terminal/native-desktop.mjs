// Installed Desktop acceptance. No passwords, tokens, or full output are saved.
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { randomBytes, createHash, privateDecrypt, constants } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
const product = process.env.PAB_ACCEPTANCE_PRODUCT || 'C:\\Program Files\\PixelsAgentBridge';
const fixture = process.env.PAB_ACCEPTANCE_FIXTURE || 'C:\\Users\\Administrator\\Downloads\\PixelsDesktopTerminalAcceptance-20261014';
const require = createRequire(process.env.PAB_ACCEPTANCE_NODE_PACKAGE || path.join(product, 'browser-runtime', 'package.json'));
const { chromium } = require('playwright-core');
const result = { device: process.env.PAB_ACCEPTANCE_TARGET || '428358431', controller: process.env.PAB_ACCEPTANCE_CONTROLLER || '90 (same-host)', source: 'native debug acceptance Desktop with installed Executor', started: new Date().toISOString(), checks: {} };
let browser, terminal;
try {
  browser = await chromium.connectOverCDP('http://127.0.0.1:39100');
  const pages = browser.contexts().flatMap(context => context.pages());
  const main = pages.find(page => page.url().includes('tauri.localhost') && !page.url().includes('desktopTerminal'));
  if (!main) throw new Error('owned main Desktop page is unavailable');
  await main.waitForLoadState('domcontentloaded');
  for (const previous of pages.filter(page => page.url().includes('desktopTerminal='))) {
    await previous.evaluate(async () => {
      await window.__TAURI_INTERNALS__.invoke('desktop_terminal_end', { sessionId: null });
      await window.__TAURI_INTERNALS__.invoke('desktop_terminal_destroy');
    }).catch(() => {});
  }
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  let scope = await main.evaluate(() => window.__TAURI_INTERNALS__.invoke('operator_current_traffic_scope'));
  if (!scope) {
    const username = 'termqa_' + Date.now().toString(36);
    const password = randomBytes(24).toString('hex');
    scope = await main.evaluate(args => window.__TAURI_INTERNALS__.invoke('operator_register_account', args), { username, password });
    fs.writeFileSync(path.join(fixture, 'owned-test-account.json'), JSON.stringify({ user_id: scope.userId, username: scope.username }));
  }
  result.account = { user_id: scope.userId, username: scope.username };
  // Read access privately on the owning computer. It never becomes a script
  // argument, tool argument, log entry, screenshot, or result-file value.
  const accessEnv = { ...process.env }; delete accessEnv.PAB_DATA_DIR;
  let sealedAccess;
  if (process.env.PAB_ACCEPTANCE_SEALED_ACCESS) {
    const keyPath = path.join(fixture, 'access-key.pem');
    const sealed = process.env.PAB_ACCEPTANCE_SEALED_ACCESS;
    const decrypted = privateDecrypt({key: fs.readFileSync(keyPath), padding: constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256'}, fs.readFileSync(sealed));
    sealedAccess = decrypted.toString('utf8'); decrypted.fill(0);
    fs.unlinkSync(keyPath); fs.unlinkSync(sealed);
  }
  const access = sealedAccess || process.env.PAB_ACCEPTANCE_REMOTE_ACCESS || execFileSync(path.join(product, 'pab-executor.exe'), ['show-access'], { env: accessEnv, windowsHide: true, encoding: 'utf8' });
  const code = access.match(/Device code:\s*(\d+)/)?.[1];
  const password = access.match(/Temporary password:\s*(\S+)/)?.[1];
  if (code !== result.device || !password) throw new Error('device identity did not match the requested acceptance target');
  await main.evaluate(args => window.__TAURI_INTERNALS__.invoke('operator_connect', args), { code, password });
  await main.evaluate(args => window.__TAURI_INTERNALS__.invoke('desktop_terminal_window', args), { code });
  const deadline = Date.now() + 30000;
  while (!terminal && Date.now() < deadline) {
    terminal = browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('desktopTerminal='));
    if (!terminal) await new Promise(resolve => setTimeout(resolve, 100));
  }
  if (!terminal) throw new Error('independent terminal window did not open');
  await terminal.waitForSelector('.desktop-terminal-setup .ant-select', { timeout: 30000 });
  const digest = createHash('sha256');
  let received = 0, tail = Buffer.alloc(0), end = null;
  await terminal.exposeBinding('observeNativeTerminal', (_, frame) => {
    if (frame.kind === 0) {
      const bytes = Buffer.from(frame.data, 'base64');
      if (frame.offset !== received) throw new Error('native renderer stream has a gap or duplicate');
      received += bytes.length; digest.update(bytes);
      tail = Buffer.concat([tail, bytes]); if (tail.length > 262144) tail = tail.subarray(tail.length - 262144);
    } else end = JSON.parse(Buffer.from(frame.data, 'base64').toString('utf8'));
  });
  await terminal.evaluate(() => {
    const acceptance = window.__PIXELS_TERMINAL_ACCEPTANCE__;
    if (!acceptance) throw new Error('native acceptance observer is not enabled');
    window.nativeAcceptance = acceptance;
    acceptance.inputProbe = [];
    acceptance.onInput = data => { acceptance.inputProbe.push({ text: data, codepoints: Array.from(data, c => c.codePointAt(0)) }); if (acceptance.inputProbe.length > 12) acceptance.inputProbe.shift(); };
    acceptance.onFrame = buffer => {
      if (buffer.byteLength >= 9) {
        const bytes = new Uint8Array(buffer), offset = Number(new DataView(buffer).getBigUint64(1));
        if (bytes[0] <= 1) {
          let binary = ''; for (const byte of bytes.subarray(9)) binary += String.fromCharCode(byte);
          void window.observeNativeTerminal({ kind: bytes[0], offset, data: btoa(binary) });
        }
      }
    };
    window.nativeAcceptance.resume = () => {
      window.nativeAcceptance.paused = false;
      const queued = window.nativeAcceptance.pending; window.nativeAcceptance.pending = []; window.nativeAcceptance.pendingBytes = 0;
      for (const callback of queued) callback();
    };
  });
  await terminal.getByRole('combobox', { name: 'Execution account' }).click();
  await terminal.locator('.ant-select-dropdown:visible').getByText('Service account', { exact: true }).click();
  await terminal.getByRole('button', { name: 'Open Shell', exact: true }).click();
  await terminal.getByText('Connected', { exact: true }).waitFor({ timeout: 30000 });
  result.checks.window_entry = true;
  result.checks.service_shell = await terminal.locator('.desktop-terminal-details').innerText();
  // Exercise keyboard through the actual xterm input path. A split output
  // marker cannot be confused with the command line echoed by ConPTY.
  await terminal.locator('.xterm-helper-textarea').focus();
  await terminal.keyboard.type("if(Get-Command Set-PSReadLineOption -ErrorAction SilentlyContinue){Set-PSReadLineOption -HistorySaveStyle SaveNothing}; $env:PIXELS_NATIVE_TEST='kept'; Write-Output ('PIXELS_'+'NATIVE_READY');");
  await terminal.keyboard.press('Enter');
  const readyDeadline = Date.now() + 15000;
  while (!tail.toString('utf8').includes('PIXELS_NATIVE_READY') && Date.now() < readyDeadline) await new Promise(resolve => setTimeout(resolve, 50));
  if (!tail.toString('utf8').includes('PIXELS_NATIVE_READY')) throw new Error('keyboard command did not execute in the real Shell');
  await terminal.screenshot({ path: path.join(fixture, 'native-terminal-service.png') });
  result.checks.keyboard_submitted = true;
  result.checks.state_marker = tail.toString('utf8').includes('PIXELS_NATIVE_READY');
  async function send(text) {
    await terminal.locator('.xterm-helper-textarea').focus(); await terminal.keyboard.insertText(text); await terminal.keyboard.press('Enter');
  }
  async function marker(text, timeout = 30000) {
    const until = Date.now() + timeout;
    while (!tail.toString('utf8').includes(text) && Date.now() < until) await new Promise(resolve => setTimeout(resolve, 25));
    if (!tail.toString('utf8').includes(text)) {
      result.stalled = { received, end, tail: tail.toString('utf8').slice(-1500), renderer: await terminal.evaluate(() => ({ paused: window.nativeAcceptance.paused, pending: window.nativeAcceptance.pendingBytes, max: window.nativeAcceptance.maxPendingBytes })) };
      throw new Error(`Shell result marker missing: ${text}`);
    }
  }
  await send("Set-Location $env:TEMP; [Console]::WriteLine(('STATE_'+$env:PIXELS_NATIVE_TEST+'_DONE')); [Console]::WriteLine(('UNICODE_'+'中'+'文 '+[char]::ConvertFromUtf32(0x1F30D)));");
  await marker('STATE_kept_DONE'); await marker('UNICODE_中文 🌍');
  result.checks.environment_and_unicode = true;
  await send("[Console]::WriteLine(('HANG_'+'START')); Start-Sleep -Seconds 60; [Console]::WriteLine(('HANG_'+'UNEXPECTED_END'));");
  await marker('HANG_START');
  await terminal.keyboard.press('Control+c');
  await send("[Console]::WriteLine(('CTRL_'+'C_CONTINUED'));"); await marker('CTRL_C_CONTINUED');
  result.checks.ctrl_c_preserves_shell = true;
  await terminal.evaluate(() => { window.nativeAcceptance.paused = true; });
  await send("$s='Y'*16384; for($i=0;$i -lt512;$i++){[Console]::WriteLine($s)}; [Console]::WriteLine(('PAUSE_'+'FINISHED'));");
  await new Promise(resolve => setTimeout(resolve, 2000));
  const pending = await terminal.evaluate(() => window.nativeAcceptance.pendingBytes);
  if (pending <= 0 || pending > 262144) throw new Error('renderer pause did not obey the output credit budget');
  await terminal.evaluate(() => window.nativeAcceptance.resume()); await marker('PAUSE_FINISHED', 180000);
  result.checks.pause_resume = { pending, max: await terminal.evaluate(() => window.nativeAcceptance.maxPendingBytes) };
  const largeStarted = Date.now(), before = received;
  await send("$s='X'*16384; for($i=0;$i -lt4500;$i++){[Console]::WriteLine($s)}; [Console]::WriteLine(('LARGE_'+'FINISHED'));");
  await marker('LARGE_FINISHED', 600000);
  if (received - before < 64 * 1024 * 1024) throw new Error('large output acceptance threshold not reached');
  result.checks.large_output = { bytes: received - before, duration_ms: Date.now() - largeStarted };
  await terminal.getByRole('button', { name: 'End session', exact: true }).click();
  await terminal.getByRole('button', { name: 'Keep using', exact: true }).click();
  await send("[Console]::WriteLine(('CANCEL_'+'KEPT_SHELL'));"); await marker('CANCEL_KEPT_SHELL');
  result.checks.cancel_close_keeps_shell = true;
  await send("[Console]::WriteLine(('TAIL_'+'FINAL')); exit 7");
  const endDeadline = Date.now() + 30000;
  while (!end && Date.now() < endDeadline) await new Promise(resolve => setTimeout(resolve, 25));
  if (!end || end.exit_code !== 7 || !tail.toString('utf8').includes('TAIL_FINAL')) throw new Error('natural exit did not preserve tail and exit status');
  const actualHash = digest.copy().digest('hex');
  if (end.output_bytes !== received || end.sha256 !== actualHash) throw new Error('actual PTY source and xterm receiver differ');
  result.checks.source_integrity = true;
  result.checks.natural_exit = end;
  // Keep only byte counts and digests. Further cases use this same observer to
  // compare the worker's actual PTY source with the data received by xterm.
  result.output = { received, digest: digest.copy().digest('hex'), sourceEnd: end };
  fs.writeFileSync(path.join(fixture, 'native-desktop-initial.json'), JSON.stringify(result, null, 2));
  console.log(JSON.stringify(result));
} catch (error) {
  if (terminal) {
    try { await terminal.screenshot({ path: path.join(fixture, 'native-terminal-error.png') }); result.visible = await terminal.locator('body').innerText(); result.inputProbe = await terminal.evaluate(() => window.nativeAcceptance?.inputProbe); } catch {}
  }
  result.error = String(error); fs.writeFileSync(path.join(fixture, 'native-desktop-initial.json'), JSON.stringify(result, null, 2));
  console.error(result.error); process.exitCode = 1;
} finally {
  await browser?.close();
}
