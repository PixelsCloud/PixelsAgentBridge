// Real short active-download desktop-process crash/reopen/recovery acceptance.
// Reuses a prepared, normally paired cross fixture's immutable synthetic source.
// start-download -> external exact-process crash -> inspect-stopped -> relaunch
// same profile -> recover-download -> independent observe-source -> finish.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, shaFile, manifest, assertManifest, unseal } from './file_transfer_cross_fixture.mjs';
const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  assert(['--phase', '--fixture', '--run-id', '--source-case', '--browser-runtime', '--report'].includes(process.argv[i]) && process.argv[i + 1] && !args.has(process.argv[i])); args.set(process.argv[i], process.argv[i + 1]);
}
assert.equal(process.platform, 'win32');
const root = fixtureRoot(args.get('--fixture')), id = args.get('--run-id'), phase = args.get('--phase');
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id));
assert(['start-download', 'inspect-stopped', 'recover-download', 'observe-source', 'finish'].includes(phase));
const marker = readJson(path.join(root, 'native-lab.json')), base = contained(root, path.join(root, 'payload', `native-restart-${id}`), phase !== 'start-download' && phase !== 'observe-source');
assert(marker.root === root && marker.machine === os.hostname());
const actor = JSON.parse(execFileSync('C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', '$w=[Security.Principal.WindowsIdentity]::GetCurrent(); @{sid=$w.User.Value;name=$w.Name;session=(Get-Process -Id $PID).SessionId}|ConvertTo-Json -Compress'], { encoding: 'utf8', timeout: 10000 }));
assert.equal(actor.sid, marker.user_sid, 'Exact fixture native user required');
const identityKeys = ['mode', 'account_id', 'account_name', 'home', 'primary_group', 'session_id', 'logon_id', 'environment_source'];
function sameIdentity(actual, expected) { for (const key of identityKeys) assert.deepEqual(actual[key] ?? null, expected[key] ?? null, `Native identity changed: ${key}`); }
if (phase === 'observe-source') {
  const reportFile = contained(root, args.get('--report')), recovery = readJson(reportFile);
  assert(recovery.run_id === id && recovery.status === 'awaiting_independent_source' && recovery.target_fixture === root);
  assert.equal(recovery.remote_identity.account_id, actor.sid); assert.equal(recovery.remote_identity.mode, 'user');
  assert.equal(recovery.remote_identity.session_id, String(actor.session)); assert.equal(recovery.executor_sha256, marker.executor_sha256);
  const db = new DatabaseSync(path.join(root, 'executor-data', 'executor.sqlite3'), { readOnly: true });
  const facts = [];
  try {
    for (const requestId of [recovery.original_attempt, recovery.resumed_item.attempt]) {
      const row = db.prepare('SELECT request_id,direction,path,size,offset,state,sha256 FROM transfer_operations WHERE request_id=?').get(requestId);
      const execution = db.prepare('SELECT request_json,context_json FROM transfer_execution WHERE request_id=?').get(requestId);
      assert(row && execution); const request = JSON.parse(execution.request_json), context = JSON.parse(execution.context_json);
      sameIdentity(context.identity, recovery.remote_identity); assert.equal(request.request_id, row.request_id); assert.equal(row.direction, 'send');
      if (requestId === recovery.resumed_item.attempt) { assert.equal(row.state, 'completed'); assert.equal(row.offset, row.size); assert.equal(request.resume_from, recovery.original_attempt); }
      assert.equal(row.path, recovery.resumed_item.spec.source.value); assert.equal(shaFile(contained(root, row.path)), recovery.expected_sha256);
      facts.push({ row, request, context });
    }
  } finally { db.close(); }
  assertManifest(manifest(contained(root, recovery.target_source)), recovery.expected_manifest, 'Independent source tree after restart');
  const observed = { schema: 1, run_id: id, pass: true, actor, host: os.hostname(), report_sha256: shaFile(reportFile), facts, source_unchanged: true };
  const out = contained(root, path.join(path.dirname(reportFile), 'source-observed.json'), false); writeJson(out, observed); console.log(JSON.stringify({ pass: true, report: out, facts })); process.exit(0);
}
if (phase === 'start-download') { assert(!fs.existsSync(base)); fs.mkdirSync(base); fs.mkdirSync(path.join(base, 'download')); }
const output = path.join(base, `${phase}.json`); assert(!fs.existsSync(output), 'Keep original phase result; never replay an unknown operation');
const start = phase === 'start-download' ? null : readJson(path.join(base, 'start-download.json'));
function findDatabase() {
  const found = [];
  function walk(directory, depth = 0) { assert(depth <= 5); for (const entry of fs.readdirSync(directory, { withFileTypes: true })) { const full = contained(root, path.join(directory, entry.name)); assert(!fs.lstatSync(full).isSymbolicLink()); if (entry.isDirectory()) walk(full, depth + 1); else if (entry.name === 'file-manager-batches.sqlite3') found.push(full); } }
  walk(path.join(root, 'operator-data')); assert.equal(found.length, 1); return found[0];
}
if (phase === 'inspect-stopped') {
  const crash = readJson(path.join(base, 'forced-stop.json')); assert(crash.pass && crash.batch_id === start.original_batch_id && crash.pid === start.desktop.pid);
  const db = new DatabaseSync(findDatabase(), { readOnly: true }); let batch, item, attempt, evidence, elapsed;
  try {
    batch = db.prepare('SELECT id,owner,spec,intent FROM fm_batches WHERE id=?').get(start.original_batch_id);
    item = db.prepare('SELECT id,batch,state,confirmed,attempt FROM fm_items WHERE id=?').get(start.active_item.spec.id);
    attempt = db.prepare('SELECT id,item,digest FROM fm_attempts WHERE id=?').get(start.active_item.attempt);
    evidence = JSON.parse(db.prepare('SELECT data FROM fm_attempt_evidence WHERE attempt=?').get(start.active_item.attempt).data);
    elapsed = db.prepare('SELECT total_ms FROM fm_elapsed WHERE batch=?').get(start.original_batch_id);
  } finally { db.close(); }
  // The durable digest/context live in the evidence table. The legacy nullable
  // fm_attempts columns are not the production recovery contract.
  assert(batch && item && attempt && item.attempt === start.active_item.attempt && item.batch === batch.id && attempt.item === item.id && evidence.digest && evidence.local_checkpoint && elapsed);
  sameIdentity(evidence.context.identity, start.remote_identity);
  const disk = manifest(path.join(base, 'download')); assert(!disk.some(row => row.path.endsWith('random.bin')));
  const parts = disk.filter(row => row.kind === 'file' && row.path.endsWith('.part')); assert.equal(parts.length, 1); assert(parts[0].bytes > 0 && parts[0].bytes < Number(start.active_item.spec.size));
  assert(BigInt(parts[0].bytes) >= BigInt(evidence.local_checkpoint));
  const result = { schema: 1, run_id: id, pass: true, status: 'original_checkpoint_retained_after_process_crash', actor, crash_sha256: shaFile(path.join(base, 'forced-stop.json')), batch, item, attempt, evidence, elapsed, disk };
  writeJson(output, result); console.log(JSON.stringify(result)); process.exit(0);
}
const sourceCase = args.get('--source-case') || start?.source_case, sourceDir = runDirectory(root, sourceCase), offer = readJson(path.join(sourceDir, 'offer.json')), target = readJson(path.join(sourceDir, 'transport', 'target-prepared.json'));
assert(offer.execution === 'user' && offer.fixture_bytes === 32 * 1024 * 1024 && target.fixture === offer.target_fixture);
if (phase === 'finish') {
  const recoveryFile = path.join(base, 'recover-download.json'), recovery = readJson(recoveryFile), observed = readJson(path.join(base, 'source-observed.json'));
  assert(recovery.status === 'awaiting_independent_source' && observed.pass && observed.run_id === id && observed.report_sha256 === shaFile(recoveryFile));
  assertManifest(manifest(path.join(base, 'download')), target.source_manifest, 'Recovered original batch final disk');
  const result = { schema: 1, run_id: id, pass: true, status: 'active_download_desktop_crash_recovery_passed', desktop_sha256: marker.desktop_sha256, executor_sha256: target.executor_sha256, original_batch_id: start.original_batch_id, original_item_id: start.active_item.spec.id, original_attempt: start.active_item.attempt, resumed_attempt: recovery.resumed_item.attempt, actions: recovery.actions, source_observation_sha256: shaFile(path.join(base, 'source-observed.json')), recovery_sha256: shaFile(recoveryFile), scope: 'One active Windows download, isolated controlling desktop killed and same profile reopened, explicit UI recovery, native user/Relay. Not every fault/direction.' };
  writeJson(output, result); console.log(JSON.stringify(result)); process.exit(0);
}
const runtime = args.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(action, predicate, message, milliseconds = 45000) { const deadline = performance.now() + milliseconds; while (performance.now() < deadline) { const value = await action(); if (predicate(value)) return value; await wait(100); } throw new Error(`Timeout: ${message}`); }
const desktop = readJson(path.join(root, 'desktop-process.json')), report = { schema: 1, run_id: id, phase, pass: false, status: 'started', actor, desktop, desktop_sha256: marker.desktop_sha256, executor_sha256: target.executor_sha256, driver_sha256: shaFile(fileURLToPath(import.meta.url)), source_case: sourceCase, target_fixture: target.fixture, target_source: target.source, expected_manifest: target.source_manifest, actions: [] };
const save = () => fs.writeFileSync(path.join(base, `${phase}-checkpoint.json`), JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
let browser;
try {
  assert.equal(shaFile(contained(root, desktop.exe)), marker.desktop_sha256);
  if (start) { const crash = readJson(path.join(base, 'forced-stop.json')); assert(crash.pass && crash.pid === start.desktop.pid && desktop.pid !== start.desktop.pid); assert(readJson(path.join(base, 'inspect-stopped.json')).pass); }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${desktop.cdp}`);
  const main = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().includes('fileManager=1')); assert(main);
  assert.equal((await invoke(main, 'operator_current_traffic_scope'))?.userId, readJson(path.join(root, 'owned-test-account.json')).user_id);
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  const access = unseal(path.join(sourceDir, 'transport', 'controller-private.pem'), readJson(path.join(sourceDir, 'transport', 'access-envelope.json')), sourceCase);
  assert(access.fixture === target.fixture && access.code === target.code && access.executor_sha256 === target.executor_sha256);
  await invoke(main, 'operator_connect', { code: access.code, password: access.password }); access.password = undefined;
  await invoke(main, 'file_manager_window', { code: target.code });
  const files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'file window');
  await until(() => files.locator('.fm-pane').count(), n => n === 2, 'both panes');
  await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).setChecked(true);
  const inventory = await invoke(files, 'file_manager_contexts', { code: target.code, forceRelay: true }); assert.equal(inventory.state, 'completed');
  const users = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id); assert.equal(users.length, 1);
  report.remote_identity = users[0].identity; if (start) sameIdentity(report.remote_identity, start.remote_identity);
  await files.getByRole('combobox', { name: 'Remote account', exact: true }).selectOption({ label: `${users[0].account_name} · ${users[0].session_id}` });
  await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
  const batches = () => invoke(files, 'file_manager_batches'), state = async batch => ({ batch: (await batches()).find(row => row.spec.id === batch), items: await invoke(files, 'file_manager_batch_items', { id: batch, after: 0, filter: null }) });
  if (phase === 'start-download') {
    assert((await batches()).every(row => !row.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled'].includes(row.phase)));
    for (const [pane, directory] of [[files.locator('.fm-pane').first(), path.join(base, 'download')], [files.locator('.fm-pane').last(), target.source]]) { const input = pane.locator('.fm-pathbar input'); await input.fill(directory); await input.press('Enter'); await until(async () => ({ value: await input.inputValue(), text: await pane.locator('.fm-pane-status [role="status"]').innerText(), errors: await pane.locator('.fm-error').count() }), row => row.value === directory && /\d+ items/.test(row.text) && !row.errors, 'owned directory'); }
    await files.getByRole('combobox', { name: 'Conflict policy', exact: true }).selectOption('ask');
    await files.locator('.fm-pane').last().getByRole('checkbox', { name: `Select ${target.tree_name}`, exact: true }).check();
    const previous = new Set((await batches()).map(row => row.spec.id)); await files.getByRole('button', { name: /^Receive(?: \(\d+\))?$/ }).click();
    report.original_batch_id = (await until(batches, rows => rows.some(row => !previous.has(row.spec.id)), 'original admission')).find(row => !previous.has(row.spec.id)).spec.id; save();
    report.partial = await until(() => state(report.original_batch_id), row => row.batch?.scan_complete && row.items.some(item => item.state === 'transferring' && BigInt(item.spec.size) === 33554432n && BigInt(item.confirmed) > 0n && BigInt(item.confirmed) < 16777216n), 'receiver-confirmed partial', 90000);
    report.active_item = report.partial.items.find(item => item.state === 'transferring' && BigInt(item.spec.size) === 33554432n && BigInt(item.confirmed) > 0n && BigInt(item.confirmed) < 16777216n);
    assert(report.active_item.attempt); report.status = 'awaiting_desktop_crash'; save(); writeJson(output, report);
  } else {
    report.original_batch_id = start.original_batch_id; report.original_attempt = start.active_item.attempt; save();
    report.reopened = await until(() => state(start.original_batch_id), row => row.batch && !row.batch.runner_active && ['reconciling', 'interrupted', 'paused'].includes(row.batch.phase), 'stale original lease recovered', 60000);
    assert.equal(report.reopened.batch.active_elapsed_ms, readJson(path.join(base, 'inspect-stopped.json')).elapsed.total_ms, 'Process downtime must not add working time');
    assert.deepEqual(report.reopened.items.map(item => item.spec.id).sort(), start.partial.items.map(item => item.spec.id).sort());
    const row = files.locator('.fm-task').filter({ has: files.getByTitle('Resume', { exact: true }) }); await until(() => row.count(), n => n === 1, 'original Resume action');
    report.actions.push('resume_original_batch_after_reopen'); save(); await row.getByTitle('Resume', { exact: true }).click();
    report.after_resume = await until(() => state(start.original_batch_id), row => row.batch && !row.batch.runner_active && ['completed', 'failed', 'partial_failed'].includes(row.batch.phase), 'original recovery or known failure', 180000); save();
    if (['failed', 'partial_failed'].includes(report.after_resume.batch.phase)) {
      // Explicit UI retry is permitted only after original observation proves a
      // known terminal failure. It must preserve the original batch/item IDs.
      report.actions.push('retry_known_failed_items_explicit_ui'); save();
      await until(() => files.getByTitle('Retry failed files', { exact: true }).count(), n => n === 1, 'known failure retry button');
      await files.getByTitle('Retry failed files', { exact: true }).click();
    }
    report.completed = await until(() => state(start.original_batch_id), row => row.batch?.phase === 'completed' && !row.batch.runner_active, 'same original batch completion', 180000);
    assert.equal(report.completed.batch.failed, '0'); assert.equal(report.completed.batch.confirmed, report.completed.batch.size);
    assert.deepEqual(report.completed.items.map(item => item.spec.id).sort(), start.partial.items.map(item => item.spec.id).sort());
    report.resumed_item = report.completed.items.find(item => item.spec.id === start.active_item.spec.id);
    assert(report.resumed_item.attempt !== start.active_item.attempt && report.resumed_item.previous_attempt === start.active_item.attempt);
    assert(BigInt(report.completed.batch.active_elapsed_ms) >= BigInt(start.partial.batch.active_elapsed_ms));
    report.expected_sha256 = target.source_manifest.find(item => item.path.endsWith('random.bin')).sha256;
    assertManifest(manifest(path.join(base, 'download')), target.source_manifest, 'Recovered download names/content/times');
    report.status = 'awaiting_independent_source'; save(); writeJson(output, report);
    await files.screenshot({ path: path.join(base, 'recovered.png') });
  }
  console.log(JSON.stringify({ run_id: id, phase, status: report.status, original_batch_id: report.original_batch_id, active_item: report.active_item && { id: report.active_item.spec.id, attempt: report.active_item.attempt, confirmed: report.active_item.confirmed }, report: output }));
} catch (error) { report.status = 'failed'; report.error = { name: error.name, message: error.message }; save(); if (!fs.existsSync(output)) writeJson(output, report); throw error; }
finally { if (browser) await browser.close(); }
