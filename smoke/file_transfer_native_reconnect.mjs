// Baseline -> externally restart ONLY the isolated target Executor -> after.
// Requires a completed file_transfer_native_cross.mjs case. Does not restart a
// machine/service, submit transfers, switch identities, or replay mutations.
// --restart-record is a Pixels-collected {root,machine,before,after} JSON pair
// of the target's original/restarted service-process.json launch observations.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, shaFile } from './file_transfer_cross_fixture.mjs';

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const option = process.argv[index], value = process.argv[index + 1];
  assert(['--phase', '--fixture', '--run-id', '--browser-runtime', '--cdp-url', '--restart-record'].includes(option) && value && !args.has(option));
  args.set(option, value);
}
const phase = args.get('--phase'); assert(['baseline', 'after'].includes(phase));
assert.equal(process.platform, 'win32');
const root = fixtureRoot(args.get('--fixture')), id = args.get('--run-id'), directory = runDirectory(root, id);
const completed = readJson(contained(root, path.join(directory, 'cross-result.json')));
const offer = readJson(contained(root, path.join(directory, 'offer.json')));
const prepared = readJson(contained(root, path.join(directory, 'transport', 'target-prepared.json')));
assert(completed.pass && completed.run_id === id && completed.controller === os.hostname());
assert(offer.execution === 'user' && prepared.host !== os.hostname());
const runtime = args.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
const endpoint = new URL(args.get('--cdp-url') || 'http://127.0.0.1:39255');
const desktop = readJson(contained(root, path.join(root, 'desktop-process.json')));
assert(endpoint.protocol === 'http:' && endpoint.hostname === '127.0.0.1' && Number(endpoint.port) === desktop.cdp);
const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
const identityKey = identity => JSON.stringify([identity.mode, identity.account_id, identity.account_name, identity.home, identity.primary_group ?? null, identity.session_id, identity.logon_id, identity.environment_source]);
const output = path.join(directory, `reconnect-${phase}.json`);
assert(!fs.existsSync(output), 'Keep the original probe; use a separate case for a new attempt');
const report = { schema: 1, run_id: id, phase, pass: false, scope: 'Native selected-user file-pane recovery after isolated target Executor restart; original completed batches remain stopped', driver_sha256: shaFile(fileURLToPath(import.meta.url)), desktop_sha256: shaFile(contained(root, desktop.exe)), started: new Date().toISOString() };
let browser, files;
try {
  browser = await chromium.connectOverCDP(endpoint.href);
  files = browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')); assert(files);
  const tabs = await invoke(files, 'file_manager_tabs'); assert(tabs.some(tab => tab.code === prepared.code));
  const remote = files.locator('.fm-pane').last(), selection = files.getByRole('combobox', { name: 'Remote account', exact: true });
  assert.equal(await remote.locator('.fm-pathbar input').inputValue(), prepared.source, 'Do not navigate or inspect unrelated directories');
  const forceRelay = offer.connection_mode === 'relay';
  assert.equal(await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).isChecked(), forceRelay);
  const inventory = await invoke(files, 'file_manager_contexts', { code: prepared.code, forceRelay });
  assert(inventory.state === 'completed' && inventory.data?.type === 'execution_contexts');
  const candidates = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection?.mode === 'user' && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id);
  assert.equal(candidates.length, 1, 'Original native user/session is unavailable; never fall back to service');
  const current = candidates[0];
  report.fresh_identity = current.identity; report.fresh_selection = current.selection;
  report.ui_selection_before_refresh = await selection.inputValue();
  const batches = await invoke(files, 'file_manager_batches');
  assert(completed.batches.every(original => batches.some(batch => batch.spec.id === original.id && batch.phase === 'completed' && !batch.runner_active)));
  report.original_batch_ids = completed.batches.map(batch => batch.id);
  if (phase === 'baseline') {
    assert.equal(report.ui_selection_before_refresh, current.selection.context_ref);
    assert.equal(await remote.locator('.fm-error').count(), 0);
    assert.match(await remote.locator('.fm-pane-status [role="status"]').innerText(), /^1 items/);
    report.status = 'ready_for_external_isolated_target_restart'; report.pass = true;
  } else {
    const baseline = readJson(contained(root, path.join(directory, 'reconnect-baseline.json')));
    assert(baseline.pass && baseline.run_id === id);
    assert.equal(identityKey(current.identity), identityKey(baseline.fresh_identity), 'Native identity changed across the restart');
    assert.notEqual(current.selection.context_ref, baseline.fresh_selection.context_ref, 'A replacement native transport must have been discovered');
    const restartFile = contained(root, args.get('--restart-record')), restart = readJson(restartFile);
    assert.equal(restart.root, prepared.fixture); assert.equal(restart.machine, prepared.host);
    for (const record of [restart.before, restart.after]) {
      assert.equal(record.exe, path.join(prepared.fixture, 'pab-executor.exe'));
      assert.equal(record.config, path.join(prepared.fixture, 'config.toml'));
    }
    assert(restart.after.started_ticks > restart.before.started_ticks, 'A new isolated Executor generation is required');
    report.restart_observation_sha256 = shaFile(restartFile);
    await remote.getByTitle('Refresh', { exact: true }).click();
    const deadline = Date.now() + 30000;
    while (Date.now() < deadline) {
      report.ui_errors = await remote.locator('.fm-error').allInnerTexts();
      report.ui_selection_after_refresh = await selection.inputValue();
      report.ui_status = await remote.locator('.fm-pane-status [role="status"]').innerText();
      // A stale-reference read can be shown briefly while the pane performs
      // bounded rediscovery. All errors must clear and the actual same-user
      // reference/directory must recover before this fixed deadline.
      if ((!report.ui_errors.length && report.ui_selection_after_refresh === current.selection.context_ref && /^1 items/.test(report.ui_status)) || report.ui_errors.some(text => !text.includes('EnvironmentChanged'))) break;
      await new Promise(resolve => setTimeout(resolve, 200));
    }
    assert.equal(report.ui_errors.length, 0, 'Native file-pane refresh failed after target restart');
    assert.equal(report.ui_selection_after_refresh, current.selection.context_ref, 'Selected native context was not renewed');
    assert.match(report.ui_status, /^1 items/);
    const after = await invoke(files, 'file_manager_batches');
    assert.deepEqual(after.map(batch => batch.spec.id).sort(), batches.map(batch => batch.spec.id).sort(), 'The probe must not admit/replay any transfer');
    assert(after.every(batch => !batch.runner_active));
    report.pass = true; report.status = 'native_user_reconnect_passed';
  }
} catch (error) {
  report.error = String(error.message || error); report.status = 'failed'; process.exitCode = 1;
} finally {
  report.finished = new Date().toISOString();
  if (files) await files.screenshot({ path: path.join(directory, `reconnect-${phase}.png`) }).catch(() => {});
  writeJson(output, report);
  await browser?.close().catch(() => {});
}
console.log(JSON.stringify({ phase, pass: report.pass, status: report.status, error: report.error || null, report: output }));
