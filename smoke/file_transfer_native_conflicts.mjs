// Short real native-window conflict acceptance on two prepared Windows hosts.
// prepare both -> exchange prepared metadata with Pixels -> run controller ->
// observe target independently -> return observation with Pixels -> finish.
// This is a bounded functional case, not a throughput or soak test.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, shaFile, unseal } from './file_transfer_cross_fixture.mjs';

const options = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  assert(['--phase', '--fixture', '--run-id', '--side', '--source-case', '--browser-runtime'].includes(process.argv[i]) && process.argv[i + 1] && !options.has(process.argv[i]));
  options.set(process.argv[i], process.argv[i + 1]);
}
assert.equal(process.platform, 'win32');
const root = fixtureRoot(options.get('--fixture')), id = options.get('--run-id'), phase = options.get('--phase');
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id));
assert(['prepare', 'run', 'observe', 'finish'].includes(phase));
const marker = readJson(path.join(root, 'native-lab.json'));
assert(marker.root === root && marker.machine === os.hostname());
const actor = JSON.parse(execFileSync('C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', '$w=[Security.Principal.WindowsIdentity]::GetCurrent(); @{sid=$w.User.Value;name=$w.Name;session=(Get-Process -Id $PID).SessionId}|ConvertTo-Json -Compress'], { encoding: 'utf8', timeout: 10000 }));
assert.equal(actor.sid, marker.user_sid, 'Explicit native user required; never fall back to service');
const base = contained(root, path.join(root, 'payload', `native-conflicts-${id}`), phase !== 'prepare');
const cases = [
  { name: 'replace', policy: 'replace', choice: 'replace' },
  { name: 'skip', policy: 'skip', choice: 'skip' },
  { name: 'keep-both', policy: 'keep_both', choice: 'keep_both' },
  { name: 'ask-replace', policy: 'ask', choice: 'replace' },
  { name: 'ask-skip', policy: 'ask', choice: 'skip' },
  { name: 'ask-keep-both', policy: 'ask', choice: 'keep_both' },
];
const collision = 'collision-中文.bin';
const nativePath = value => {
  assert(value && ['utf8', 'windows_wide'].includes(value.encoding));
  return value.encoding === 'utf8' ? value.value : Buffer.from(value.value, 'base64').toString('utf16le');
};
function filesIn(folder) {
  return fs.readdirSync(folder).sort().map(name => {
    const file = contained(root, path.join(folder, name)), stat = fs.lstatSync(file);
    assert(stat.isFile() && !stat.isSymbolicLink() && stat.size <= 65536, 'Only bounded ordinary case files');
    return { name, bytes: stat.size, sha256: shaFile(file) };
  });
}
if (phase === 'prepare') {
  const side = options.get('--side'); assert(['local', 'remote'].includes(side));
  assert(!fs.existsSync(base)); fs.mkdirSync(base);
  for (const item of cases) for (const role of ['source', 'destination']) {
    const directory = path.join(base, item.name, role); fs.mkdirSync(directory, { recursive: true });
    fs.writeFileSync(path.join(directory, collision), randomBytes(role === 'source' ? 4096 : 3072), { flag: 'wx' });
  }
  const prepared = { schema: 1, run_id: id, root, side, host: os.hostname(), actor, native_home: os.homedir(), desktop_sha256: marker.desktop_sha256, executor_sha256: marker.executor_sha256,
    cases: cases.map(item => ({ ...item, source: filesIn(path.join(base, item.name, 'source')), destination: filesIn(path.join(base, item.name, 'destination')) })) };
  writeJson(path.join(base, 'prepared.json'), prepared);
  console.log(JSON.stringify({ phase, run_id: id, side, actor, prepared: path.join(base, 'prepared.json') })); process.exit(0);
}
const preparedFile = path.join(base, 'prepared.json'), prepared = readJson(preparedFile);
assert(prepared.run_id === id && prepared.root === root && prepared.actor.sid === actor.sid);
function verifySources() {
  for (const item of prepared.cases) assert.deepEqual(filesIn(path.join(base, item.name, 'source')), item.source, 'Source changed');
}
function verifyDestination(item, source, original, destinationRoot) {
  const directory = contained(root, path.join(destinationRoot, item.name, 'destination'));
  const actual = filesIn(directory), destination = path.join(directory, collision);
  assert.equal(item.items.length, 1);
  const entry = item.items[0], chosen = nativePath(entry.effective_destination || entry.spec.destination);
  assert.equal(path.dirname(chosen), directory, 'Effective destination escaped its case directory');
  if (item.choice === 'skip') {
    assert.equal(entry.state, 'skipped'); assert.equal(item.final.phase, 'completed_with_skips');
    assert.deepEqual(actual, original, 'Skip changed original destination');
  } else if (item.choice === 'replace') {
    assert.equal(entry.state, 'completed'); assert.equal(item.final.phase, 'completed');
    assert.equal(chosen, destination); assert.deepEqual(actual, source, 'Replace saved incorrect bytes');
  } else {
    assert.equal(entry.state, 'completed'); assert.equal(item.final.phase, 'completed');
    assert.notEqual(chosen, destination); assert.equal(actual.length, 2);
    assert.deepEqual(actual.find(row => row.name === collision), original[0], 'Keep both changed original');
    const added = actual.find(row => row.name !== collision); assert.equal(path.join(directory, added.name), chosen);
    assert.equal(added.bytes, source[0].bytes); assert.equal(added.sha256, source[0].sha256);
  }
  return { case: item.name, choice: item.choice, batch_id: item.batch_id, item_id: entry.spec.id, attempt: entry.attempt, actual_files: actual };
}
if (phase === 'observe') {
  assert.equal(prepared.side, 'remote'); verifySources();
  const runFile = path.join(base, 'controller-run.json'), run = readJson(runFile);
  assert(run.status === 'awaiting_independent_target' && run.run_id === id && run.target_fixture === root);
  assert.equal(run.target_prepared_sha256, shaFile(preparedFile)); assert.equal(run.executor_sha256, marker.executor_sha256);
  assert.equal(run.remote_identity.mode, 'user'); assert.equal(run.remote_identity.account_id, actor.sid); assert.equal(run.remote_identity.session_id, String(actor.session));
  const sourceMetadata = readJson(path.join(base, 'controller-prepared.json'));
  assert.equal(run.controller_prepared_sha256, shaFile(path.join(base, 'controller-prepared.json')));
  const rows = run.batches.filter(item => item.direction === 'upload'); assert.equal(rows.length, cases.length);
  const observations = rows.map(item => verifyDestination(item, sourceMetadata.cases.find(row => row.name === item.name).source, prepared.cases.find(row => row.name === item.name).destination, base));
  const result = { schema: 1, run_id: id, pass: true, actor, host: os.hostname(), report_sha256: shaFile(runFile), prepared_sha256: shaFile(preparedFile), observations, source_bytes_preserved: true };
  writeJson(path.join(base, 'target-observed.json'), result); console.log(JSON.stringify(result)); process.exit(0);
}
assert.equal(prepared.side, 'local'); verifySources();
if (phase === 'finish') {
  const runFile = path.join(base, 'controller-run.json'), run = readJson(runFile), observedFile = path.join(base, 'target-observed.json'), observed = readJson(observedFile);
  assert(run.run_id === id && run.status === 'awaiting_independent_target' && observed.pass && observed.run_id === id);
  assert.equal(observed.report_sha256, shaFile(runFile)); assert.equal(observed.prepared_sha256, run.target_prepared_sha256);
  assert.deepEqual(observed.actor.sid, run.remote_identity.account_id); assert.equal(observed.observations.length, cases.length);
  const targetPrepared = readJson(path.join(base, 'target-prepared.json'));
  const downloads = run.batches.filter(item => item.direction === 'download'); assert.equal(downloads.length, cases.length);
  const local = downloads.map(item => verifyDestination(item, targetPrepared.cases.find(row => row.name === item.name).source, prepared.cases.find(row => row.name === item.name).destination, base));
  assert.equal(new Set(run.batches.map(item => item.batch_id)).size, cases.length * 2);
  const result = { schema: 1, run_id: id, pass: true, status: 'native_conflict_policies_passed', desktop_sha256: marker.desktop_sha256, executor_sha256: run.executor_sha256, driver_sha256: run.driver_sha256, controller_run_sha256: shaFile(runFile), target_observed_sha256: shaFile(observedFile), batches: run.batches.map(({ name, direction, batch_id, items, final }) => ({ name, direction, batch_id, phase: final.phase, item_ids: items.map(item => item.spec.id) })), local_observations: local, remote_observations: observed.observations, scope: 'Both directions: replace, skip, keep both, ask with each resolution. Not late conflicts/apply-to-rest/all functional acceptance.' };
  writeJson(path.join(base, 'finished.json'), result); console.log(JSON.stringify(result)); process.exit(0);
}
const sourceCase = options.get('--source-case'), original = runDirectory(root, sourceCase), offer = readJson(path.join(original, 'offer.json')), target = readJson(path.join(original, 'transport', 'target-prepared.json'));
const targetPreparedFile = path.join(base, 'target-prepared.json'), targetPrepared = readJson(targetPreparedFile);
assert(targetPrepared.root === target.fixture && targetPrepared.side === 'remote' && targetPrepared.run_id === id && targetPrepared.actor.sid === offer.remote_user.account_id);
const output = path.join(base, 'controller-run.json'), checkpoint = path.join(base, 'controller-checkpoint.json');
assert(!fs.existsSync(output) && !fs.existsSync(checkpoint), 'Inspect original IDs; never replay a previously admitted run');
const runtime = options.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(action, predicate, message, milliseconds = 45000) {
  const deadline = performance.now() + milliseconds;
  while (performance.now() < deadline) { const value = await action(); if (predicate(value)) return value; await wait(150); }
  throw new Error(`Timeout: ${message}`);
}
const report = { schema: 1, run_id: id, pass: false, status: 'started', actor, host: os.hostname(), target_fixture: target.fixture, controller_prepared_sha256: shaFile(preparedFile), target_prepared_sha256: shaFile(targetPreparedFile), desktop_sha256: marker.desktop_sha256, executor_sha256: target.executor_sha256, driver_sha256: shaFile(fileURLToPath(import.meta.url)), batches: [], checks: {} };
const save = () => fs.writeFileSync(checkpoint, JSON.stringify(report, null, 2) + '\n', { mode: 0o600 });
let browser;
try {
  const desktop = readJson(path.join(root, 'desktop-process.json')); assert.equal(shaFile(contained(root, desktop.exe)), marker.desktop_sha256);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${desktop.cdp}`);
  const main = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().includes('fileManager=1')); assert(main);
  assert.equal((await invoke(main, 'operator_current_traffic_scope'))?.userId, readJson(path.join(root, 'owned-test-account.json')).user_id);
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  const access = unseal(path.join(original, 'transport', 'controller-private.pem'), readJson(path.join(original, 'transport', 'access-envelope.json')), sourceCase);
  assert(access.code === target.code && access.fixture === target.fixture && access.executor_sha256 === target.executor_sha256);
  await invoke(main, 'operator_connect', { code: access.code, password: access.password }); access.password = undefined;
  await invoke(main, 'file_manager_window', { code: target.code });
  const files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'file window');
  await until(() => files.locator('.fm-pane').count(), count => count === 2, 'both panes');
  assert((await invoke(files, 'file_manager_batches')).every(batch => !batch.runner_active));
  await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).setChecked(true);
  const inventory = await invoke(files, 'file_manager_contexts', { code: target.code, forceRelay: true }); assert.equal(inventory.state, 'completed');
  const selected = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id);
  assert.equal(selected.length, 1); assert.equal(selected[0].identity.mode, 'user');
  assert.equal(selected[0].identity.home, targetPrepared.native_home);
  assert.equal(selected[0].identity.environment_source, 'native_account');
  assert.equal(selected[0].identity.primary_group, null);
  report.remote_identity = selected[0].identity;
  await files.getByRole('combobox', { name: 'Remote account', exact: true }).selectOption({ label: `${selected[0].account_name} · ${selected[0].session_id}` });
  report.link = await invoke(files, 'file_manager_link_status', { code: target.code, forceRelay: true }); assert(report.link.connected && report.link.path === 'relay');
  await files.getByRole('combobox', { name: 'Transfer filter', exact: true }).selectOption('all');
  const local = files.locator('.fm-pane').first(), remote = files.locator('.fm-pane').last(), remoteBase = path.join(target.fixture, 'payload', `native-conflicts-${id}`);
  const navigate = async (pane, directory) => {
    for (const checkbox of await pane.locator('.fm-file-row input:checked').all()) await checkbox.uncheck();
    const input = pane.locator('.fm-pathbar input'); await input.fill(directory); await input.press('Enter');
    await until(async () => ({ value: await input.inputValue(), text: await pane.locator('.fm-pane-status [role="status"]').innerText(), errors: await pane.locator('.fm-error').count() }), state => state.value === directory && /\d+ items/.test(state.text) && !state.errors, 'case directory');
    await pane.getByRole('checkbox', { name: `Select ${collision}`, exact: true }).waitFor();
  };
  for (const direction of ['upload', 'download']) for (const item of cases) {
    await navigate(local, path.join(base, item.name, direction === 'upload' ? 'source' : 'destination'));
    await navigate(remote, path.join(remoteBase, item.name, direction === 'upload' ? 'destination' : 'source'));
    await files.getByRole('combobox', { name: 'Conflict policy', exact: true }).selectOption(item.policy);
    await (direction === 'upload' ? local : remote).getByRole('checkbox', { name: `Select ${collision}`, exact: true }).check();
    const previous = new Set((await invoke(files, 'file_manager_batches')).map(batch => batch.spec.id));
    await files.getByRole('button', { name: direction === 'upload' ? /^Send(?: \(\d+\))?$/ : /^Receive(?: \(\d+\))?$/ }).click();
    const batch = await until(() => invoke(files, 'file_manager_batches'), rows => rows.some(row => !previous.has(row.spec.id)), 'original batch admission');
    const admitted = batch.find(row => !previous.has(row.spec.id)), entry = { ...item, direction, batch_id: admitted.spec.id };
    report.batches.push(entry); save();
    assert.equal(admitted.spec.conflict, item.policy); assert.equal(admitted.spec.direction, direction); assert.equal(admitted.spec.execution.mode, 'user'); assert(admitted.spec.force_relay);
    if (item.policy === 'ask') {
      const waiting = await until(() => invoke(files, 'file_manager_batches'), rows => rows.some(row => row.spec.id === entry.batch_id && row.phase === 'waiting_conflict' && !row.runner_active), 'ask stops at conflict');
      entry.waiting = waiting.find(row => row.spec.id === entry.batch_id); save();
      const details = files.getByRole('dialog', { name: 'Transfer details', exact: true });
      // Newly admitted batch is first; all earlier case batches have stopped.
      await until(() => files.locator('.fm-task').count(), count => count === batch.length, 'visible admitted task');
      await files.locator('.fm-task').first().getByRole('button', { name: 'Details', exact: true }).click();
      await details.getByRole('button', { name: 'Replace', exact: true }).waitFor();
      await details.getByTitle('Close details', { exact: true }).click();
      const unresolved = (await invoke(files, 'file_manager_batch_items', { id: entry.batch_id, after: 0, filter: null }));
      assert.equal(unresolved.length, 1); assert.equal(unresolved[0].state, 'waiting_conflict');
      assert.equal((await invoke(files, 'file_manager_batches')).find(row => row.spec.id === entry.batch_id).phase, 'waiting_conflict');
      entry.closed_without_resolving = true; save();
      await files.locator('.fm-task').first().getByRole('button', { name: 'Details', exact: true }).click();
      await details.getByRole('button', { name: { replace: 'Replace', skip: 'Skip', keep_both: 'Keep both' }[item.choice], exact: true }).click();
      await details.getByTitle('Close details', { exact: true }).click();
    }
    const finals = await until(() => invoke(files, 'file_manager_batches'), rows => rows.some(row => row.spec.id === entry.batch_id && !row.runner_active && ['completed', 'completed_with_skips', 'partial_failed', 'failed', 'cancelled', 'unconfirmed'].includes(row.phase)), 'original conflict batch terminal');
    entry.final = finals.find(row => row.spec.id === entry.batch_id);
    entry.items = await invoke(files, 'file_manager_batch_items', { id: entry.batch_id, after: 0, filter: null }); save();
    assert.equal(entry.final.failed, '0'); assert.equal(entry.items.length, 1); assert.equal(entry.final.phase, item.choice === 'skip' ? 'completed_with_skips' : 'completed');
    assert.equal(entry.items[0].state, item.choice === 'skip' ? 'skipped' : 'completed');
    if (direction === 'download') verifyDestination(entry, targetPrepared.cases.find(row => row.name === item.name).source, prepared.cases.find(row => row.name === item.name).destination, base);
  }
  verifySources(); assert((await invoke(files, 'file_manager_batches')).every(batch => !batch.runner_active));
  report.checks.six_cases_both_directions = true; report.checks.ask_close_preserves_pending = true;
  report.status = 'awaiting_independent_target';
  await files.screenshot({ path: path.join(base, 'finished.png') }); save(); writeJson(output, report);
  console.log(JSON.stringify({ status: report.status, run_id: id, batches: report.batches.map(({ name, direction, batch_id, final }) => ({ name, direction, batch_id, phase: final.phase })), report: output }));
} catch (error) {
  report.status = 'failed'; report.error = { name: error.name, message: error.message }; save();
  if (!fs.existsSync(output)) writeJson(output, report); throw error;
} finally { if (browser) await browser.close(); }
