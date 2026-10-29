// Explicit native Tauri acceptance against an isolated, already running candidate.
// Uses normal account registration/pairing and real IPC; no mock window or server.
// Requires the candidate launcher records, an installed Playwright runtime, and
// --fixture <absolute owned root> --browser-runtime <absolute runtime directory>.
// Optional: --cdp-url http://127.0.0.1:39255 . Reports never contain credentials.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const options = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  assert(process.argv[i]?.startsWith('--') && process.argv[i + 1], 'Use named option/value pairs');
  assert(!options.has(process.argv[i]), 'Duplicate option');
  options.set(process.argv[i], process.argv[i + 1]);
}
for (const key of options.keys()) assert(['--fixture', '--browser-runtime', '--cdp-url'].includes(key), 'Unknown option');
const fixture = options.get('--fixture');
const runtime = options.get('--browser-runtime');
assert(fixture && runtime && path.isAbsolute(fixture) && path.isAbsolute(runtime), 'Explicit absolute fixture/runtime paths required');
const root = fs.realpathSync(fixture);
const endpoint = new URL(options.get('--cdp-url') || 'http://127.0.0.1:39255');
assert(endpoint.protocol === 'http:' && endpoint.hostname === '127.0.0.1', 'Only an owned loopback debug listener is accepted');
const readJson = name => JSON.parse(fs.readFileSync(path.join(root, name), 'utf8').replace(/^\uFEFF/, ''));
const inside = name => {
  const resolved = fs.realpathSync(name);
  const relative = path.relative(root, resolved);
  assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative), 'Candidate path escaped fixture');
  return resolved;
};
const service = readJson('service-process.json');
const desktop = readJson('desktop-process.json');
const executor = inside(service.exe);
const clientConfig = inside(service.config);
inside(desktop.exe); inside(desktop.config);
assert.equal(Number(endpoint.port), desktop.cdp);
const require = createRequire(import.meta.url);
const { chromium } = require(path.join(runtime, 'node_modules', 'playwright-core'));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const shaFile = name => digest(fs.readFileSync(name));
const runId = randomUUID();
const payload = path.join(root, 'payload', `native-${runId}`);
fs.mkdirSync(payload, { recursive: false });
const source = path.join(payload, 'source');
const upload = path.join(payload, 'upload');
const download = path.join(payload, 'download');
for (const directory of [source, upload, download]) fs.mkdirSync(directory);
const tree = 'folder-中文';
fs.mkdirSync(path.join(source, tree, 'nested'), { recursive: true });
fs.mkdirSync(path.join(source, tree, 'empty'));
fs.writeFileSync(path.join(source, tree, 'zero.bin'), Buffer.alloc(0), { flag: 'wx' });
fs.writeFileSync(path.join(source, tree, 'nested', 'random.bin'), randomBytes(8 * 1024 * 1024), { flag: 'wx' });
fs.writeFileSync(path.join(source, tree, 'nested', 'unicode-空格 name.txt'), 'Synthetic native file-transfer acceptance\n', { flag: 'wx' });

function manifest(base) {
  const rows = [];
  function walk(relative) {
    for (const name of fs.readdirSync(path.join(base, relative)).sort()) {
      const child = path.join(relative, name);
      const stat = fs.lstatSync(path.join(base, child));
      assert(!stat.isSymbolicLink(), 'Unexpected link in owned fixture');
      if (stat.isDirectory()) { rows.push({ path: child, kind: 'directory' }); walk(child); }
      else { assert(stat.isFile()); rows.push({ path: child, kind: 'file', bytes: stat.size, sha256: shaFile(path.join(base, child)) }); }
    }
  }
  walk(''); return rows;
}
const expected = manifest(source);
const report = {
  scope: 'One-host native Windows candidate, real account/pairing/Tauri IPC, UI-driven upload/download and independent final manifests. Not cross-host, full-platform, fault, Relay, performance or soak acceptance.',
  run_id: runId, started: new Date().toISOString(), payload,
  driver_sha256: shaFile(fileURLToPath(import.meta.url)),
  environment: { platform: process.platform, architecture: process.arch, node: process.version },
  candidates: { desktop: shaFile(desktop.exe), executor: shaFile(executor) },
  checks: {}, batches: [], expected_manifest: expected,
};
let browser, main, files, password, accessPassword;
let failure;
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
const invoke = (page, command, args = {}) => page.evaluate(({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args), { command, args });
async function until(action, predicate, description, milliseconds = 30000) {
  const deadline = Date.now() + milliseconds;
  let observed;
  while (Date.now() < deadline) {
    observed = await action();
    if (predicate(observed)) return observed;
    await wait(200);
  }
  throw new Error(`Timeout: ${description}`);
}
async function navigate(pane, destination) {
  await pane.getByTitle('Edit path', { exact: true }).click();
  const input = pane.locator('.fm-pathbar input');
  await input.waitFor({ state: 'visible' });
  await input.fill(destination); await input.press('Enter');
  await until(async () => ({ path: await pane.locator('.fm-breadcrumbs button').last().getAttribute('title'), status: await pane.locator('.fm-pane-status [role="status"]').innerText() }), value => value.path === destination && /\d+ items/.test(value.status), 'owned directory navigation');
  assert.equal(await pane.locator('.fm-error').count(), 0, 'Owned directory produced an error');
}
async function transfer(button) {
  const previous = new Set((await invoke(files, 'file_manager_batches')).map(row => row.spec.id));
  await button.click();
  const admitted = await until(() => invoke(files, 'file_manager_batches'), rows => rows.some(row => !previous.has(row.spec.id)), 'UI batch admission');
  const batch = admitted.find(row => !previous.has(row.spec.id));
  report.batches.push({ id: batch.spec.id, direction: batch.spec.direction, admitted_phase: batch.phase });
  const final = await until(() => invoke(files, 'file_manager_batches'), rows => rows.some(row => row.spec.id === batch.spec.id && !row.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled', 'unconfirmed'].includes(row.phase)), 'original batch terminal state and coordinator stop', 120000);
  const result = final.find(row => row.spec.id === batch.spec.id);
  report.batches.at(-1).final = result;
  assert.equal(result.phase, 'completed', 'Actual native batch did not complete');
}
try {
  browser = await chromium.connectOverCDP(endpoint.toString());
  main = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('tauri.localhost') && !new URL(page.url()).search), Boolean, 'owned main Tauri page');
  await main.waitForLoadState('domcontentloaded');
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  let scope = await invoke(main, 'operator_current_traffic_scope');
  if (scope) {
    const owned = readJson('owned-test-account.json');
    assert.equal(scope.userId, owned.user_id, 'Refuse to reuse an unrelated account');
  } else {
    const username = `ftqa_${Date.now().toString(36)}_${randomBytes(3).toString('hex')}`;
    password = randomBytes(24).toString('hex');
    scope = await invoke(main, 'operator_register_account', { username, password });
    fs.writeFileSync(path.join(root, 'owned-test-account.json'), JSON.stringify({ user_id: scope.userId, username: scope.username }) + '\n', { flag: 'wx', mode: 0o600 });
  }
  assert(scope?.userId && scope.username, 'Normal account is not signed in');
  report.account = { user_id: scope.userId, username: scope.username };
  report.checks.normal_account = true;
  const childEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('PAB_')));
  let access;
  await until(async () => {
    try { access = execFileSync(executor, ['--config', clientConfig, 'show-access'], { env: childEnv, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], timeout: 10000 }); return true; }
    catch { return false; }
  }, Boolean, 'owned candidate access');
  let text = access.toString('utf8'); access.fill(0);
  const code = text.match(/Device code:\s*(\d{9})/)?.[1];
  accessPassword = text.match(/Temporary password:\s*(\S+)/)?.[1]; text = '';
  assert(code && accessPassword, 'Owned candidate access could not be parsed');
  report.target_code = code;
  await invoke(main, 'operator_connect', { code, password: accessPassword });
  accessPassword = undefined;
  const label = await invoke(main, 'file_manager_window', { code });
  files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'independent native file window');
  await files.locator('.fm-pane').first().waitFor();
  await until(() => files.locator('.fm-pane').count(), count => count === 2, 'both native file panes');
  assert.equal(await files.locator('.fm-pane').count(), 2);
  assert.equal(await invoke(main, 'file_manager_window', { code }), label, 'Repeated open created another owner');
  report.checks.native_independent_window = true;
  report.checks.repeated_open_same_owner = true;
  const tabs = await invoke(files, 'file_manager_tabs');
  assert.equal(tabs.length, 1); assert.equal(tabs[0].code, code);
  const previousBatches = await invoke(files, 'file_manager_batches');
  assert(previousBatches.every(row => !row.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled'].includes(row.phase)), 'Reconcile earlier owned work before this acceptance run');
  const previouslyCompleted = previousBatches.filter(row => row.phase === 'completed').length;
  await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
  await until(() => files.locator('.fm-task >span[title="completed"]').count(), count => count === previouslyCompleted, 'initial visible task history');
  const local = files.locator('.fm-pane').first(), remote = files.locator('.fm-pane').last();
  await navigate(local, source); await navigate(remote, upload);
  await files.screenshot({ path: path.join(payload, 'native-owned-directories.png') });
  await local.getByRole('checkbox', { name: `Select ${tree}`, exact: true }).check();
  await transfer(files.getByRole('button', { name: /^Send(?: \(\d+\))?$/, exact: true }));
  assert.deepEqual(manifest(upload), expected, 'Independent uploaded directory manifest differs');
  report.checks.ui_upload_manifest = true;
  await remote.getByRole('checkbox', { name: `Select ${tree}`, exact: true }).waitFor();
  report.checks.native_receive_directory_auto_refresh = true;
  await navigate(local, download);
  await remote.getByRole('checkbox', { name: `Select ${tree}`, exact: true }).check();
  await transfer(files.getByRole('button', { name: /^Receive(?: \(\d+\))?$/, exact: true }));
  assert.deepEqual(manifest(download), expected, 'Independent downloaded directory manifest differs');
  report.checks.ui_download_manifest = true;
  await local.getByRole('checkbox', { name: `Select ${tree}`, exact: true }).waitFor();
  report.checks.native_download_directory_auto_refresh = true;
  await until(() => files.locator('.fm-task >span[title="completed"]').count(), count => count === previouslyCompleted + 2, 'both visible completed task states');
  report.checks.both_visible_completed = true;
  report.link = await invoke(files, 'file_manager_link_status', { code, forceRelay: false });
  assert(report.link.connected, 'No actual link observation');
  const mainDenied = await invoke(main, 'file_manager_tabs').then(() => false, () => true);
  report.checks.main_cannot_claim_file_owner = mainDenied;
  let operatorDenied = false;
  try { await invoke(files, 'operator_current_traffic_scope'); } catch { operatorDenied = true; }
  report.checks.file_window_cannot_call_operator_api = operatorDenied;
  await files.screenshot({ path: path.join(payload, 'native-transfer-completed.png') });
  assert(mainDenied, 'Main window could claim file owner');
  assert(operatorDenied, 'File window can call unrelated operator APIs');
  report.pass = true;
} catch (error) {
  let message = String(error?.message || error);
  for (const secret of [password, accessPassword]) if (secret) message = message.replaceAll(secret, '[redacted]');
  report.pass = false; report.error = message; failure = error;
} finally {
  // Payload is retained with original IDs for diagnosis/reconciliation; do not
  // remove files while a failed/unknown native batch might still reference them.
  report.finished = new Date().toISOString();
  fs.writeFileSync(path.join(payload, 'native-result.json'), JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
  fs.writeFileSync(path.join(root, 'latest-native-result.json'), JSON.stringify({ report: path.join(payload, 'native-result.json'), pass: report.pass, run_id: runId }) + '\n', { mode: 0o600 });
  await browser?.close().catch(() => {});
}
console.log(JSON.stringify({ pass: report.pass, run_id: runId, checks: report.checks, error: report.error || null, report: path.join(payload, 'native-result.json') }));
if (failure) process.exitCode = 1;
