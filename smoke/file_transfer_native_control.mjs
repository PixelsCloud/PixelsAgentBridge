// Real Windows native GUI control stages on a prepared cross-host fixture.
// pause-upload -> independent target pause observation -> resume-upload ->
// independent target completion observation -> cancel-download -> independent
// target cancellation observation -> finish. Intermediate stages are not PASS.
// No mutation is automatically replayed when an observation fails.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { randomBytes } from 'node:crypto';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, manifest, assertManifest, shaFile, unseal } from './file_transfer_cross_fixture.mjs';

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index], value = process.argv[index + 1];
  assert(['--phase', '--fixture', '--run-id', '--browser-runtime', '--cdp-url', '--suite'].includes(key) && value && !args.has(key));
  args.set(key, value);
}
const phase = args.get('--phase');
const suite = args.get('--suite') || 'forward'; assert(['forward', 'reverse'].includes(suite));
const pauseDirection = suite === 'forward' ? 'upload' : 'download', cancelDirection = suite === 'forward' ? 'download' : 'upload';
const pausePhase = `pause-${pauseDirection}`, resumePhase = `resume-${pauseDirection}`, cancelPhase = `cancel-${cancelDirection}`;
assert([pausePhase, resumePhase, cancelPhase, 'finish'].includes(phase));
assert.equal(process.platform, 'win32');
const root = fixtureRoot(args.get('--fixture')), id = args.get('--run-id'), directory = runDirectory(root, id);
const offerFile = contained(root, path.join(directory, 'offer.json')), offer = readJson(offerFile);
const preparedFile = contained(root, path.join(directory, 'transport', 'target-prepared.json')), prepared = readJson(preparedFile);
assert(offer.run_id === id && prepared.run_id === id && offer.controller_machine === os.hostname() && prepared.host !== os.hostname());
assert(offer.execution === 'user' && offer.connection_mode === 'relay' && offer.fixture_bytes === 32 * 1024 * 1024);
assert.equal(prepared.fixture, offer.target_fixture); assert.equal(prepared.offer_sha256, shaFile(offerFile));
const desktop = readJson(path.join(root, 'desktop-process.json'));
const endpoint = new URL(args.get('--cdp-url') || 'http://127.0.0.1:39255');
assert(endpoint.protocol === 'http:' && endpoint.hostname === '127.0.0.1' && Number(endpoint.port) === desktop.cdp);
const runtime = args.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
const output = path.join(directory, `control-${phase}.json`);
assert(!fs.existsSync(output), 'Keep the original stage; reconcile its original IDs before another run');
const report = { schema: 1, run_id: id, phase, suite, pause_direction: pauseDirection, cancel_direction: cancelDirection, pass: false, status: 'started', scope: 'Real-account explicit native-user 32MiB forced-Relay controls and durable active time; independent target observations required', driver_sha256: shaFile(fileURLToPath(import.meta.url)), desktop_sha256: shaFile(contained(root, desktop.exe)), target_executor_sha256: prepared.executor_sha256, offer_sha256: shaFile(offerFile), prepared_sha256: shaFile(preparedFile), batches: [], checks: {}, started: new Date().toISOString() };
const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action, predicate, description, milliseconds = 30000) {
  const deadline = performance.now() + milliseconds;
  while (performance.now() < deadline) { const value = await action(); if (predicate(value)) return value; await wait(100); }
  throw new Error(`Timeout: ${description}`);
}
const stopped = batch => !batch.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled'].includes(batch.phase);
const readStage = name => {
  const file = contained(root, path.join(directory, `control-${name}.json`)), stage = readJson(file);
  assert(stage.run_id === id && stage.status !== 'failed'); return stage;
};
const observation = name => {
  const file = contained(root, path.join(directory, 'transport', `target-control-${name}.json`)), value = readJson(file);
  assert(value.pass && value.run_id === id && value.host === prepared.host && value.offer_sha256 === shaFile(offerFile));
  return { file, value };
};
let browser, files, accessSecret;
try {
  browser = await chromium.connectOverCDP(endpoint.href);
  const main = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().includes('fileManager=1')); assert(main);
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  const accountFile = path.join(root, 'owned-test-account.json');
  let account = await invoke(main, 'operator_current_traffic_scope');
  if (!account && phase === pausePhase && !fs.existsSync(accountFile)) {
    accessSecret = randomBytes(24).toString('hex');
    account = await invoke(main, 'operator_register_account', { username: `ftqa_${Date.now().toString(36)}_${randomBytes(3).toString('hex')}`, password: accessSecret });
    accessSecret = undefined; writeJson(accountFile, { user_id: account.userId, username: account.username });
  }
  assert.equal(account?.userId, readJson(accountFile).user_id);
  if (phase === pausePhase) {
    const access = unseal(contained(root, path.join(directory, 'transport', 'controller-private.pem')), readJson(contained(root, path.join(directory, 'transport', 'access-envelope.json'))), id);
    assert(access.code === prepared.code && access.host === prepared.host && access.fixture === prepared.fixture && access.executor_sha256 === prepared.executor_sha256);
    accessSecret = access.password;
    await invoke(main, 'operator_connect', { code: prepared.code, password: accessSecret });
    access.password = undefined; accessSecret = undefined;
    report.checks.normal_account_and_pairing = true;
  }
  await invoke(main, 'file_manager_window', { code: prepared.code });
  files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'owned native file window');
  await until(() => files.locator('.fm-pane').count(), count => count === 2, 'two native panes');
  const batches = () => invoke(files, 'file_manager_batches');
  const current = async batchId => ({ batch: (await batches()).find(batch => batch.spec.id === batchId), items: await invoke(files, 'file_manager_batch_items', { id: batchId, after: 0 }) });
  const activeRow = title => files.locator('.fm-task').filter({ has: files.getByTitle(title, { exact: true }) });
  const local = files.locator('.fm-pane').first(), remote = files.locator('.fm-pane').last();
  if (phase === pausePhase) {
    assert((await batches()).every(stopped), 'Reconcile all earlier original batches first');
    await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
    await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).setChecked(true);
    const inventory = await invoke(files, 'file_manager_contexts', { code: prepared.code, forceRelay: true });
    assert(inventory.state === 'completed' && inventory.data?.type === 'execution_contexts');
    const users = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection?.mode === 'user' && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id);
    assert.equal(users.length, 1, 'Exact sampled user/session required; no service fallback');
    report.selected_identity = users[0].identity;
    await files.getByRole('combobox', { name: 'Remote account', exact: true }).selectOption({ label: `${users[0].account_name} · ${users[0].session_id}` });
  }
  assert(await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).isChecked());
  async function navigate(pane, target) {
    const input = pane.locator('.fm-pathbar input'); await input.fill(target); await input.press('Enter');
    await until(async () => ({ path: await input.inputValue(), status: await pane.locator('.fm-pane-status [role="status"]').innerText(), errors: await pane.locator('.fm-error').count() }), value => value.path === target && /\d+ items/.test(value.status) && !value.errors, 'owned directory navigation');
  }
  async function start(direction) {
    const prior = new Set((await batches()).map(batch => batch.spec.id));
    await files.getByRole('button', { name: direction === 'upload' ? /^Send(?: \(\d+\))?$/ : /^Receive(?: \(\d+\))?$/ }).click();
    const admitted = (await until(batches, rows => rows.some(batch => !prior.has(batch.spec.id)), 'original UI batch admission')).find(batch => !prior.has(batch.spec.id));
    report.batches.push({ id: admitted.spec.id, direction });
    fs.writeFileSync(path.join(directory, `control-${phase}-checkpoint.json`), JSON.stringify({ run_id: id, batches: report.batches }) + '\n', { mode: 0o600 });
    assert(admitted.spec.force_relay && admitted.spec.execution.mode === 'user' && admitted.spec.direction === direction);
    return admitted.spec.id;
  }
  async function partial(batchId) {
    const value = await until(() => current(batchId), state => state.batch?.scan_complete && state.items.some(item => item.state === 'transferring' && item.attempt && BigInt(item.spec.size) === 32n * 1024n * 1024n && BigInt(item.confirmed) > 0n && BigInt(item.confirmed) < BigInt(item.spec.size)), 'partially receiver-confirmed original file', 90000);
    return { ...value, active_item: value.items.find(item => item.state === 'transferring' && BigInt(item.spec.size) === 32n * 1024n * 1024n && BigInt(item.confirmed) > 0n && BigInt(item.confirmed) < BigInt(item.spec.size)) };
  }
  if (phase === pausePhase) {
    await navigate(local, path.join(directory, pauseDirection === 'upload' ? 'source' : 'download')); await navigate(remote, pauseDirection === 'upload' ? prepared.receive : prepared.source);
    await (pauseDirection === 'upload' ? local : remote).getByRole('checkbox', { name: `Select ${pauseDirection === 'upload' ? offer.tree_name : prepared.tree_name}`, exact: true }).check();
    const batchId = await start(pauseDirection); report.partial = await partial(batchId);
    await until(() => activeRow('Pause').count(), count => count === 1, 'one active original UI row');
    const requested = performance.now(); await activeRow('Pause').getByTitle('Pause', { exact: true }).click();
    report.paused = await until(() => current(batchId), state => state.batch?.phase === 'paused' && !state.batch.runner_active, 'acknowledged original pause', 5000);
    report.pause_ack_ms = performance.now() - requested; assert(report.pause_ack_ms <= 5000, 'Pause acknowledgement exceeded 5 seconds');
    report.active_item = report.paused.items.find(item => item.spec.id === report.partial.active_item.spec.id);
    assert(/^\d+$/.test(report.paused.batch.active_elapsed_ms) && BigInt(report.paused.batch.active_elapsed_ms) > 0n, 'Durable working time is missing');
    report.active_elapsed_ms = report.paused.batch.active_elapsed_ms;
    assert(report.active_item.state === 'paused' && report.active_item.attempt === report.partial.active_item.attempt);
    assert(BigInt(report.active_item.confirmed) >= BigInt(report.partial.active_item.confirmed) && BigInt(report.active_item.confirmed) < BigInt(report.active_item.spec.size));
    report.stable_samples = [];
    const stableSince = performance.now();
    for (let sample = 0; sample < 6; sample++) {
      if (sample) await wait(1000);
      const state = await current(batchId), item = state.items.find(item => item.spec.id === report.active_item.spec.id);
      assert(state.batch.phase === 'paused' && !state.batch.runner_active && state.batch.confirmed === report.paused.batch.confirmed && item.confirmed === report.active_item.confirmed && item.attempt === report.active_item.attempt && state.batch.active_elapsed_ms === report.active_elapsed_ms);
      report.stable_samples.push({ elapsed_ms: performance.now() - stableSince, batch_confirmed: state.batch.confirmed, item_confirmed: item.confirmed, state: item.state, active_elapsed_ms: state.batch.active_elapsed_ms });
    }
    assert(report.stable_samples.at(-1).elapsed_ms >= 5000);
    await activeRow('Resume').waitFor();
    assert.equal(await activeRow('Resume').getByLabel('Transfer speed', { exact: true }).innerText(), '—');
    assert.equal(await activeRow('Resume').getByLabel('Estimated remaining', { exact: true }).innerText(), '—');
    report.elapsed_label = await activeRow('Resume').getByLabel('Active transfer time', { exact: true }).innerText();
    assert(/^Active (?!—)/.test(report.elapsed_label) && !report.elapsed_label.includes('ms'));
    if (pauseDirection === 'download') report.local_partial_manifest = manifest(path.join(directory, 'download'));
    report.status = 'paused_waiting_independent_target';
  } else if (phase === resumePhase) {
    const pause = readStage(pausePhase), remotePause = observation('pause');
    report.selected_identity = pause.selected_identity;
    assert.equal(remotePause.value.control_sha256, shaFile(path.join(directory, `control-${pausePhase}.json`)));
    const batchId = pause.batches[0].id; report.batches = pause.batches;
    assert((await current(batchId)).batch.phase === 'paused');
    assert.equal((await current(batchId)).batch.active_elapsed_ms, pause.active_elapsed_ms);
    if (pauseDirection === 'download') assertManifest(manifest(path.join(directory, 'download')), pause.local_partial_manifest, 'Paused original local checkpoint remains unchanged');
    assert.equal(await activeRow('Resume').count(), 1);
    await activeRow('Resume').getByTitle('Resume', { exact: true }).click();
    report.completed = await until(() => current(batchId), state => state.batch?.phase === 'completed' && !state.batch.runner_active, 'original resumed batch completion', 180000);
    assert(report.completed.batch.failed === '0' && report.completed.batch.confirmed === report.completed.batch.size);
    assert(BigInt(report.completed.batch.active_elapsed_ms) > BigInt(pause.active_elapsed_ms), 'Resume must add working time to the original batch');
    assert.deepEqual(report.completed.items.map(item => item.spec.id).sort(), pause.paused.items.map(item => item.spec.id).sort());
    const resumed = report.completed.items.find(item => item.spec.id === pause.active_item.spec.id);
    assert(resumed.state === 'completed' && resumed.confirmed === resumed.spec.size && resumed.previous_attempt === pause.active_item.attempt && resumed.attempt !== pause.active_item.attempt);
    report.active_item = resumed; report.pause_report_sha256 = shaFile(path.join(directory, `control-${pausePhase}.json`));
    if (pauseDirection === 'download') { report.local_received_manifest = manifest(path.join(directory, 'download')); assertManifest(report.local_received_manifest, prepared.source_manifest, 'Resumed local download SHA and times'); }
    report.status = `${pauseDirection}_complete_waiting_independent_target`;
  } else if (phase === cancelPhase) {
    const resume = readStage(resumePhase), upload = observation('completed');
    report.selected_identity = resume.selected_identity;
    assert.equal(upload.value.control_sha256, shaFile(path.join(directory, `control-${resumePhase}.json`)));
    assert((await batches()).every(stopped));
    await navigate(remote, cancelDirection === 'upload' ? prepared.receive : prepared.source); await navigate(local, path.join(directory, cancelDirection === 'upload' ? 'source' : 'download'));
    await (cancelDirection === 'upload' ? local : remote).getByRole('checkbox', { name: `Select ${cancelDirection === 'upload' ? offer.tree_name : prepared.tree_name}`, exact: true }).check();
    const batchId = await start(cancelDirection); report.partial = await partial(batchId);
    const requested = performance.now(); assert.equal(await activeRow('Cancel').count(), 1);
    await activeRow('Cancel').getByTitle('Cancel', { exact: true }).click();
    report.cancelled = await until(() => current(batchId), state => state.batch?.phase === 'cancelled' && !state.batch.runner_active, 'acknowledged original cancellation', 5000);
    report.cancel_ack_ms = performance.now() - requested; assert(report.cancel_ack_ms <= 5000, 'Cancel acknowledgement exceeded 5 seconds');
    assert(/^\d+$/.test(report.cancelled.batch.active_elapsed_ms) && BigInt(report.cancelled.batch.active_elapsed_ms) > 0n);
    report.active_item = report.cancelled.items.find(item => item.spec.id === report.partial.active_item.spec.id);
    assert(report.active_item.state === 'cancelled' && report.active_item.attempt === report.partial.active_item.attempt && BigInt(report.active_item.confirmed) < BigInt(report.active_item.spec.size));
    const before = report.cancelled.batch.confirmed; await wait(5000);
    const settled = await current(batchId); assert(settled.batch.phase === 'cancelled' && !settled.batch.runner_active && settled.batch.confirmed === before && settled.batch.active_elapsed_ms === report.cancelled.batch.active_elapsed_ms);
    for (const item of report.partial.items.filter(item => item.state === 'completed')) assert(settled.items.some(actual => actual.spec.id === item.spec.id && actual.state === 'completed' && actual.confirmed === item.confirmed));
    const again = await invoke(files, 'file_manager_batch_intent', { id: batchId, intent: 'cancel' }); assert(again.phase === 'cancelled' && !again.runner_active);
    if (cancelDirection === 'download') {
      const finalFile = path.join(directory, 'download', prepared.tree_name, 'nested', 'random.bin'); assert(!fs.existsSync(finalFile), 'Cancelled random file must not be publicly published');
      report.local_partial_manifest = manifest(path.join(directory, 'download'));
    }
    report.original_completed_id = resume.batches[0].id; report.status = 'cancelled_waiting_independent_target';
  } else {
    const pause = readStage(pausePhase), resume = readStage(resumePhase), cancel = readStage(cancelPhase);
    const observations = ['pause', 'completed', 'cancelled'].map(observation);
    for (let index = 0; index < observations.length; index++) assert.equal(observations[index].value.control_sha256, shaFile(path.join(directory, `control-${[pausePhase, resumePhase, cancelPhase][index]}.json`)));
    const ids = [pause.batches[0].id, cancel.batches[0].id], final = await batches();
    assert(ids.every(batchId => final.some(batch => batch.spec.id === batchId && !batch.runner_active)));
    assert.equal(final.find(batch => batch.spec.id === ids[0]).phase, 'completed'); assert.equal(final.find(batch => batch.spec.id === ids[1]).phase, 'cancelled');
    assertManifest(manifest(path.join(directory, 'source')), offer.expected_manifest, 'Controller original source unchanged');
    assertManifest(manifest(path.join(directory, 'download')), cancelDirection === 'download' ? cancel.local_partial_manifest : prepared.source_manifest, 'Original local received files/checkpoint stopped changing');
    report.batches = [pause.batches[0], cancel.batches[0]]; report.observations = observations.map(value => ({ file: path.basename(value.file), sha256: shaFile(value.file) }));
    report.pause_ack_ms = pause.pause_ack_ms; report.cancel_ack_ms = cancel.cancel_ack_ms;
    report.checks = { normal_account_and_pairing: pause.checks.normal_account_and_pairing === true, acknowledged_partial_pause: true, paused_checkpoint_stable_on_both_hosts: true, same_batch_and_item_resume: true, linked_original_transfer_attempt: true, resumed_receiver_sha_and_times: true, original_cancelled_peer_stopped: true, completed_items_retained: true, cancelled_final_not_published: true, cancelled_partial_unchanged: true, durable_active_time_freezes_and_resumes: true };
    assert(Object.values(report.checks).every(value => value === true));
    report.pass = true; report.status = 'control_cases_passed';
  }
  report.link = await invoke(files, 'file_manager_link_status', { code: prepared.code, forceRelay: true }); assert(report.link.connected && report.link.path === 'relay');
} catch (error) {
  report.error = String(error.message || error); if (accessSecret) report.error = report.error.replaceAll(accessSecret, '[redacted]'); report.status = 'failed'; report.pass = false; process.exitCode = 1;
} finally {
  report.finished = new Date().toISOString();
  if (files) await files.screenshot({ path: path.join(directory, `control-${phase}.png`) }).catch(() => {});
  writeJson(output, report); await browser?.close().catch(() => {});
}
console.log(JSON.stringify({ phase, status: report.status, pass: report.pass, error: report.error || null, report: output, batches: report.batches }));
