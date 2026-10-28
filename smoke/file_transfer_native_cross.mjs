// Real two-Windows-host native GUI acceptance, using normal account/pairing.
// prepare -> Pixels metadata/key transport -> target prepare -> run -> target
// observe -> Pixels observation transport -> finish. run is NOT a full PASS.
// No password, token or private key is an argument or report field.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { randomBytes } from 'node:crypto';
import { fixtureRoot, runDirectory, prepareController, manifest, assertManifest, readJson, writeJson, contained, shaFile, unseal } from './file_transfer_cross_fixture.mjs';

const args = new Map();
const allowed = ['--phase', '--fixture', '--browser-runtime', '--cdp-url', '--run-id', '--target-machine', '--target-fixture', '--target-executor-sha', '--connection-mode', '--execution', '--remote-account', '--remote-sid', '--remote-session', '--remote-logon', '--remote-owner-sid', '--fixture-bytes'];
for (let i = 2; i < process.argv.length; i += 2) {
  assert(allowed.includes(process.argv[i]) && process.argv[i + 1] && !args.has(process.argv[i]), 'Unique named option/value pairs required');
  args.set(process.argv[i], process.argv[i + 1]);
}
assert.equal(process.platform, 'win32', 'This adapter covers Windows pairs');
const root = fixtureRoot(args.get('--fixture'));
const id = args.get('--run-id');
const directory = runDirectory(root, id);
const phase = args.get('--phase');
if (phase === 'prepare') {
  const user = args.get('--execution') === 'user' ? { account_name: args.get('--remote-account'), account_id: args.get('--remote-sid'), session_id: args.get('--remote-session'), logon_id: args.get('--remote-logon'), token_owner_sid: args.get('--remote-owner-sid') } : null;
  const result = prepareController(root, id, args.get('--target-machine'), args.get('--target-fixture'), args.get('--target-executor-sha'), args.get('--connection-mode') || 'auto', args.get('--execution') || 'service', user, args.has('--fixture-bytes') ? Number(args.get('--fixture-bytes')) : undefined);
  console.log(JSON.stringify({ status: 'controller_prepared', run_id: id, directory, offer: path.join(directory, 'offer.json'), public_key: path.join(directory, 'transport', 'controller-public.pem'), controller_machine: result.offer.controller_machine }));
} else if (['run', 'finish'].includes(phase)) {
  const runtime = args.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
  const endpoint = new URL(args.get('--cdp-url') || 'http://127.0.0.1:39255');
  assert(endpoint.protocol === 'http:' && endpoint.hostname === '127.0.0.1');
  const desktop = readJson(path.join(root, 'desktop-process.json'));
  contained(root, desktop.exe); contained(root, desktop.config); assert.equal(Number(endpoint.port), desktop.cdp);
  const offerFile = contained(root, path.join(directory, 'offer.json'));
  const offer = readJson(offerFile), preparedFile = contained(root, path.join(directory, 'transport', 'target-prepared.json'));
  const prepared = readJson(preparedFile);
  assert(offer.schema === 1 && prepared.schema === 1 && offer.run_id === id && prepared.run_id === id);
  assert.equal(offer.controller_machine, os.hostname()); assert.notEqual(prepared.host, os.hostname());
  assert.equal(prepared.host, offer.target_machine); assert.equal(prepared.fixture, offer.target_fixture);
  assert.equal(prepared.executor_sha256, offer.target_executor_sha256); assert.equal(prepared.offer_sha256, shaFile(offerFile));
  const remoteDirectory = path.join(offer.target_fixture, 'payload', `cross-${id}`);
  assert.equal(prepared.receive, path.join(remoteDirectory, 'receive')); assert.equal(prepared.source, path.join(remoteDirectory, 'source'));
  const localSource = path.join(directory, 'source'), localDownload = path.join(directory, 'download');
  const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
  const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
  async function until(action, predicate, description, milliseconds = 30000) {
    const deadline = Date.now() + milliseconds;
    while (Date.now() < deadline) { const value = await action(); if (predicate(value)) return value; await wait(200); }
    throw new Error(`Timeout: ${description}`);
  }
  const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
  let browser, files, password;
  const forceRelay = offer.connection_mode === 'relay';
  const record = { schema: 1, scope: 'Two different Windows computers; real normal-account native Tauri UI and pairing. Independent receiver observation required before PASS; not the entire FT matrix.', run_id: id, controller: os.hostname(), target: prepared.host, connection_mode: offer.connection_mode, execution: offer.execution, driver_sha256: shaFile(fileURLToPath(import.meta.url)), fixture_helper_sha256: shaFile(fileURLToPath(new URL('./file_transfer_cross_fixture.mjs', import.meta.url))), desktop_sha256: shaFile(desktop.exe), target_executor_sha256: prepared.executor_sha256, offer_sha256: shaFile(offerFile), prepared_sha256: shaFile(preparedFile), checks: {}, batches: [], started: new Date().toISOString() };
  let output = path.join(directory, phase === 'run' ? 'controller-run.json' : 'cross-result.json');
  try {
    if (fs.existsSync(output)) throw new Error('This phase already has an outcome; inspect the original run instead of replaying it');
    browser = await chromium.connectOverCDP(endpoint.toString());
    const main = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('tauri.localhost') && !new URL(page.url()).search), Boolean, 'owned main window');
    await main.waitForLoadState('domcontentloaded'); await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
    if (phase === 'run') {
      assertManifest(manifest(localSource), offer.expected_manifest, 'Original controller source');
      let scope = await invoke(main, 'operator_current_traffic_scope');
      if (scope) assert.equal(scope.userId, readJson(path.join(root, 'owned-test-account.json')).user_id, 'Unrelated account');
      else {
        const username = `ftqa_${Date.now().toString(36)}_${randomBytes(3).toString('hex')}`;
        password = randomBytes(24).toString('hex'); scope = await invoke(main, 'operator_register_account', { username, password });
        writeJson(path.join(root, 'owned-test-account.json'), { user_id: scope.userId, username: scope.username });
      }
      assert(scope?.userId); record.account = { user_id: scope.userId, username: scope.username }; record.checks.normal_account = true;
      const access = unseal(contained(root, path.join(directory, 'transport', 'controller-private.pem')), readJson(contained(root, path.join(directory, 'transport', 'access-envelope.json'))), id);
      assert.equal(access.host, prepared.host); assert.equal(access.fixture, prepared.fixture); assert.equal(access.executor_sha256, prepared.executor_sha256); assert.equal(access.code, prepared.code);
      assert(/^\d{9}$/.test(access.code) && typeof access.password === 'string' && access.password.length > 0);
      password = access.password;
      const connected = await invoke(main, 'operator_connect', { code: access.code, password });
      access.password = undefined; password = undefined;
      record.paired_device = connected; record.checks.normal_pairing = true;
      const label = await invoke(main, 'file_manager_window', { code: prepared.code });
      files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'independent native file window');
      await until(() => files.locator('.fm-pane').count(), count => count === 2, 'both native panes');
      assert.equal(await invoke(main, 'file_manager_window', { code: prepared.code }), label); record.checks.repeated_open_same_owner = true;
      assert((await invoke(files, 'file_manager_tabs')).some(tab => tab.code === prepared.code));
      await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).setChecked(forceRelay);
      const selection = files.getByRole('combobox', { name: 'Remote account', exact: true });
      if (offer.execution === 'user') {
        const inventory = await invoke(files, 'file_manager_contexts', { code: prepared.code, forceRelay });
        assert(inventory.state === 'completed' && inventory.data?.type === 'execution_contexts');
        const selected = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id);
        assert.equal(selected.length, 1, 'The explicitly sampled target native session is unavailable/changed');
        record.selected_execution_identity = selected[0].identity;
        await selection.selectOption({ label: `${selected[0].account_name}${selected[0].session_id ? ` · ${selected[0].session_id}` : ''}` });
      } else await selection.selectOption('service');
      const local = files.locator('.fm-pane').first(), remote = files.locator('.fm-pane').last();
      async function navigate(pane, destination) {
        const input = pane.locator('.fm-pathbar input'); await input.fill(destination); await input.press('Enter');
        await until(async () => ({ path: await input.inputValue(), status: await pane.locator('.fm-pane-status [role="status"]').innerText() }), value => value.path === destination && /\d+ items/.test(value.status), 'owned directory navigation');
        assert.equal(await pane.locator('.fm-error').count(), 0);
      }
      const previous = await invoke(files, 'file_manager_batches');
      assert(previous.every(batch => !batch.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled'].includes(batch.phase)), 'Earlier work must be reconciled first');
      const completedBefore = previous.filter(batch => batch.phase === 'completed').length;
      await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
      async function transfer(button, direction) {
        const prior = new Set((await invoke(files, 'file_manager_batches')).map(batch => batch.spec.id));
        await button.click();
        const rows = await until(() => invoke(files, 'file_manager_batches'), values => values.some(batch => !prior.has(batch.spec.id)), 'UI batch admission');
        const admitted = rows.find(batch => !prior.has(batch.spec.id));
        record.batches.push({ id: admitted.spec.id, direction });
        // Preserve original IDs even when a later observer/network/renderer fails.
        fs.writeFileSync(path.join(directory, 'controller-checkpoint.json'), JSON.stringify({ run_id: id, batches: record.batches, phase: direction }) + '\n', { mode: 0o600 });
        assert.equal(admitted.spec.direction, direction); assert.equal(admitted.spec.execution.mode, offer.execution); assert.equal(admitted.spec.force_relay, forceRelay);
        const final = await until(() => invoke(files, 'file_manager_batches'), values => values.some(batch => batch.spec.id === admitted.spec.id && !batch.runner_active && ['completed', 'completed_with_skips', 'failed', 'partial_failed', 'cancelled', 'unconfirmed'].includes(batch.phase)), 'original batch terminal state', 180000);
        const result = final.find(batch => batch.spec.id === admitted.spec.id); record.batches.at(-1).final = result;
        assert.equal(result.phase, 'completed', 'Native cross-host batch did not complete'); assert.equal(result.failed, '0'); assert.equal(result.confirmed, result.size);
      }
      await navigate(local, localSource); await navigate(remote, prepared.receive);
      await local.getByRole('checkbox', { name: `Select ${offer.tree_name}`, exact: true }).check();
      await transfer(files.getByRole('button', { name: /^Send(?: \(\d+\))?$/ }), 'upload');
      await remote.getByRole('checkbox', { name: `Select ${offer.tree_name}`, exact: true }).waitFor(); record.checks.remote_receive_auto_refresh = true;
      await navigate(remote, prepared.source); await navigate(local, localDownload);
      await remote.getByRole('checkbox', { name: `Select ${prepared.tree_name}`, exact: true }).check();
      await transfer(files.getByRole('button', { name: /^Receive(?: \(\d+\))?$/ }), 'download');
      assertManifest(manifest(localDownload), prepared.source_manifest, 'Independent target-origin download'); record.checks.download_manifest_and_times = true;
      await local.getByRole('checkbox', { name: `Select ${prepared.tree_name}`, exact: true }).waitFor(); record.checks.local_receive_auto_refresh = true;
      await until(() => files.locator('.fm-task >span[title="completed"]').count(), count => count === completedBefore + 2, 'visible original task completion'); record.checks.visible_completed_states = true;
      const directoryTime = prepared.source_manifest.find(entry => entry.path === prepared.tree_name && entry.kind === 'directory').modified_at_unix_ms;
      const displayedTime = await files.evaluate(value => new Date(value).toLocaleString(undefined, { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }), directoryTime);
      // The directory first appears before its final metadata is applied. Wait
      // for the coalesced automatic refresh, not just the first visible name.
      await until(() => local.locator('time').first().innerText(), value => value === displayedTime, 'automatically refreshed final directory metadata');
      record.checks.directory_final_metadata_auto_refresh = true;
      record.link = await invoke(files, 'file_manager_link_status', { code: prepared.code, forceRelay });
      assert(record.link.connected); assert.equal(record.link.path, forceRelay ? 'relay' : 'p2p', 'Actual path differs from the requested acceptance case');
      record.checks.actual_connection_path = true;
      const denied = await invoke(files, 'operator_current_traffic_scope').then(() => false, () => true); assert(denied); record.checks.file_window_operator_api_denied = true;
      await files.screenshot({ path: path.join(directory, 'cross-native-completed.png') });
      record.pass = false; record.status = 'awaiting_independent_target_observation';
    } else {
      const priorFile = contained(root, path.join(directory, 'controller-run.json')), prior = readJson(priorFile);
      assert.equal(prior.status, 'awaiting_independent_target_observation'); assert.equal(prior.run_id, id); assert.equal(prior.batches.length, 2);
      const observedFile = contained(root, path.join(directory, 'transport', 'target-observed.json')), observed = readJson(observedFile);
      assert(observed.pass && observed.run_id === id && observed.host === prepared.host && observed.code === prepared.code);
      assert.equal(observed.offer_sha256, shaFile(offerFile)); assert.equal(observed.prepared_sha256, shaFile(preparedFile));
      assertManifest(observed.received_manifest, offer.expected_manifest, 'Independent other-computer receiver');
      assertManifest(observed.source_manifest, prepared.source_manifest, 'Independent other-computer source');
      assertManifest(manifest(localSource), offer.expected_manifest, 'Controller source after transfer');
      assertManifest(manifest(localDownload), prepared.source_manifest, 'Controller final download');
      if (offer.execution === 'user') assert(observed.selected_token_owner_verified && observed.receive_owners.every(entry => entry.owner_sid === offer.remote_user.token_owner_sid));
      files = browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')); assert(files);
      const batches = await invoke(files, 'file_manager_batches');
      assert(prior.batches.every(original => batches.some(batch => batch.spec.id === original.id && batch.phase === 'completed' && !batch.runner_active)), 'An original cross-host batch is not complete/stopped');
      record.batches = prior.batches; record.checks = { ...prior.checks, independent_target_disk_manifest_and_times: true, source_still_unchanged: true, original_batches_stopped: true, selected_native_token_owner: offer.execution === 'user' ? observed.selected_token_owner_verified : null };
      record.link = prior.link; record.remote_observation_sha256 = shaFile(observedFile); record.original_run_report_sha256 = shaFile(priorFile);
      record.pass = true; record.status = 'passed';
    }
  } catch (error) {
    let text = String(error.message || error); if (password) text = text.replaceAll(password, '[redacted]');
    record.pass = false; record.status = 'failed'; record.error = text; process.exitCode = 1;
  } finally {
    record.finished = new Date().toISOString();
    if (!fs.existsSync(output)) writeJson(output, record);
    await browser?.close().catch(() => {});
  }
  console.log(JSON.stringify({ pass: record.pass, status: record.status, run_id: id, checks: record.checks, error: record.error || null, report: output }));
} else assert.fail('Explicit prepare, run, or finish phase required');
