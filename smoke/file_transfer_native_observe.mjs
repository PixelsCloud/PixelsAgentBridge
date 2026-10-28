// Read-only final consistency check for a completed isolated native run.
// Rechecks the ORIGINAL batch IDs, three on-disk manifests, and visible UI.
// --fixture <owned root> --browser-runtime <runtime> [--cdp-url <loopback URL>]
// No account, pairing, file operation, or transfer is replayed.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createHash, randomUUID } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  assert(['--fixture', '--browser-runtime', '--cdp-url'].includes(process.argv[i]) && process.argv[i + 1] && !args.has(process.argv[i]), 'Expected unique named option/value pairs');
  args.set(process.argv[i], process.argv[i + 1]);
}
assert(path.isAbsolute(args.get('--fixture') || '') && path.isAbsolute(args.get('--browser-runtime') || ''), 'Absolute owned fixture/runtime required');
const root = fs.realpathSync(args.get('--fixture'));
const endpoint = new URL(args.get('--cdp-url') || 'http://127.0.0.1:39255');
assert(endpoint.protocol === 'http:' && endpoint.hostname === '127.0.0.1', 'Owned loopback listener required');
function contained(name) {
  const result = fs.realpathSync(name), relative = path.relative(root, result);
  assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative), 'Path escaped owned fixture');
  return result;
}
const readJson = name => JSON.parse(fs.readFileSync(name, 'utf8').replace(/^\uFEFF/, ''));
const sha = name => createHash('sha256').update(fs.readFileSync(name)).digest('hex');
const desktop = readJson(path.join(root, 'desktop-process.json'));
contained(desktop.exe); contained(desktop.config);
assert.equal(Number(endpoint.port), desktop.cdp);
const original = contained(readJson(path.join(root, 'latest-native-result.json')).report);
const previous = readJson(original);
assert(previous.pass && previous.batches.length === 2, 'Original native run did not succeed');
const payload = contained(previous.payload);
assert.equal(path.dirname(original), payload);
assert(previous.expected_manifest.filter(row => row.kind === 'file').reduce((sum, row) => sum + row.bytes, 0) <= 32 * 1024 * 1024, 'This bounded observer is not a large-file performance probe');
function manifest(base) {
  const rows = [];
  function walk(relative) {
    for (const name of fs.readdirSync(path.join(base, relative)).sort()) {
      const child = path.join(relative, name), full = path.join(base, child), stat = fs.lstatSync(full);
      assert(!stat.isSymbolicLink(), 'Unexpected link');
      if (stat.isDirectory()) { rows.push({ path: child, kind: 'directory' }); walk(child); }
      else { assert(stat.isFile()); rows.push({ path: child, kind: 'file', bytes: stat.size, sha256: sha(full) }); }
    }
  }
  walk(''); return rows;
}
const result = { scope: 'Read-only native UI and final-file consistency for two original batches; no transfer replay or full FT claim', run_id: previous.run_id, source_report_sha256: sha(original), observer_sha256: sha(fileURLToPath(import.meta.url)), batch_ids: previous.batches.map(batch => batch.id), checks: {}, started: new Date().toISOString() };
const output = path.join(payload, `native-ui-observation-${randomUUID()}`);
let browser;
try {
  for (const name of ['source', 'upload', 'download']) assert.deepEqual(manifest(contained(path.join(payload, name))), previous.expected_manifest, `${name} final manifest differs`);
  result.checks.three_independent_final_manifests = true;
  const { chromium } = createRequire(import.meta.url)(path.join(args.get('--browser-runtime'), 'node_modules', 'playwright-core'));
  browser = await chromium.connectOverCDP(endpoint.toString());
  const files = browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1'));
  assert(files, 'Owned file window is absent');
  const invoke = (command, values = {}) => files.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
  const tabs = await invoke('file_manager_tabs');
  assert(tabs.some(tab => tab.code === previous.target_code));
  const batches = await invoke('file_manager_batches');
  assert(result.batch_ids.every(id => batches.some(batch => batch.spec.id === id && batch.phase === 'completed' && !batch.runner_active)), 'An original batch is not durably complete/stopped');
  assert(batches.every(batch => !batch.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled'].includes(batch.phase)), 'Earlier owned work is still active or uncertain');
  result.checks.original_batches_complete_without_runner = true;
  const local = files.locator('.fm-pane').first(), remote = files.locator('.fm-pane').last();
  assert.equal(await local.locator('.fm-pathbar input').inputValue(), path.join(payload, 'download'));
  assert.equal(await remote.locator('.fm-pathbar input').inputValue(), path.join(payload, 'upload'));
  for (const pane of [local, remote]) await pane.getByRole('checkbox', { name: 'Select folder-中文', exact: true }).waitFor({ timeout: 10000 });
  result.checks.both_receive_directories_auto_refreshed = true;
  await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
  const completed = batches.filter(batch => batch.phase === 'completed').length;
  const deadline = Date.now() + 10000;
  while (await files.locator('.fm-task >span[title="completed"]').count() !== completed && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 100));
  assert.equal(await files.locator('.fm-task >span[title="completed"]').count(), completed);
  assert.equal(await files.locator('.fm-task button[title="Pause"]').count(), 0);
  result.checks.visible_completed_states = true;
  await files.screenshot({ path: `${output}.png` });
  result.pass = true;
} catch (error) { result.pass = false; result.error = String(error.message || error); process.exitCode = 1; }
finally {
  result.finished = new Date().toISOString();
  fs.writeFileSync(`${output}.json`, JSON.stringify(result, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  await browser?.close().catch(() => {});
}
console.log(JSON.stringify({ pass: result.pass, checks: result.checks, error: result.error || null, report: `${output}.json` }));
