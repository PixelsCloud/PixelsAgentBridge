// Real native file-window mutations on prepared Windows candidate profiles.
// prepare on each native user -> create/rename/delete on controller -> observe
// each phase independently on target -> finish. An intermediate phase is not PASS.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, shaFile, unseal } from './file_transfer_cross_fixture.mjs';

const options = new Map();
for (let i = 2; i < process.argv.length; i += 2) {
  assert(['--phase', '--fixture', '--run-id', '--side', '--source-case', '--target-fixture', '--browser-runtime', '--stage', '--ui-report'].includes(process.argv[i]) && process.argv[i + 1] && !options.has(process.argv[i]));
  options.set(process.argv[i], process.argv[i + 1]);
}
assert.equal(process.platform, 'win32');
const root = fixtureRoot(options.get('--fixture')), id = options.get('--run-id');
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id));
const phase = options.get('--phase');
assert(['prepare', 'create', 'recover-create', 'rename', 'delete', 'observe', 'finish'].includes(phase));
const base = contained(root, path.join(root, 'payload', `native-ops-${id}`), phase !== 'prepare');
const marker = readJson(path.join(root, 'native-lab.json'));
assert(marker.root === root && marker.machine === os.hostname());
const actor = JSON.parse(execFileSync('C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', '$w=[Security.Principal.WindowsIdentity]::GetCurrent(); @{sid=$w.User.Value;name=$w.Name;session=(Get-Process -Id $PID).SessionId}|ConvertTo-Json -Compress'], { encoding: 'utf8', timeout: 10000 }));
assert.equal(actor.sid, marker.user_sid, 'Use the explicitly selected native user, never service fallback');
const createdName = `created-中文-${id.slice(0, 8)}`, renamedName = `renamed-空格 ${id.slice(0, 8)}`, cancelledName = `cancelled-${id.slice(0, 8)}`;
const folder = path.join(base, 'files');
function directoryIdentity(filename) {
  const metadata = fs.lstatSync(filename, { bigint: true }); assert(metadata.isDirectory() && !metadata.isSymbolicLink());
  return { dev: String(metadata.dev), ino: String(metadata.ino), birth_ns: String(metadata.birthtimeNs) };
}
function seeds(prepared) {
  for (const entry of prepared.seeds) {
    const filename = contained(root, path.join(folder, entry.name));
    const metadata = fs.lstatSync(filename);
    assert(metadata.isFile() && !metadata.isSymbolicLink() && metadata.size === entry.bytes);
    assert.equal(shaFile(filename), entry.sha256, 'Seed data changed');
  }
}
const actionFor = stage => ({ create: 'create_directory', rename: 'rename', delete: 'delete' })[stage];
function localMutation(stage) {
  const matches = [];
  function walk(directory, depth = 0) {
    assert(depth <= 5, 'Unexpected operator profile nesting');
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const filename = contained(root, path.join(directory, entry.name));
      assert(!fs.lstatSync(filename).isSymbolicLink());
      if (entry.isDirectory()) walk(filename, depth + 1);
      else if (entry.name === 'file-manager-local.sqlite3') {
        const database = new DatabaseSync(filename, { readOnly: true });
        try {
          for (const row of database.prepare("SELECT id,spec,reply FROM local_manager_requests WHERE spec LIKE ? AND json_extract(spec,'$.action')=?").all(`%${id}%`, actionFor(stage))) matches.push({ request_id: row.id, operation: JSON.parse(row.spec), reply: row.reply && JSON.parse(row.reply), journal: filename });
        } finally { database.close(); }
      }
    }
  }
  walk(path.join(root, 'operator-data')); assert.equal(matches.length, 1, 'Exactly one original local mutation, including failures');
  const row = matches[0]; assert.equal(row.reply?.request_id, row.request_id); assert.equal(row.reply.result.result, 'applied');
  return { request_id: row.request_id, endpoint: 'local', action: actionFor(stage), operation: row.operation, state: 'completed', result: 'applied', journal: row.journal };
}
function remoteMutation(stage) {
  const database = new DatabaseSync(path.join(root, 'executor-data', 'executor.sqlite3'), { readOnly: true });
  let rows;
  try {
    rows = database.prepare("SELECT f.request_id,e.request_json,e.context_json,f.reply_json FROM filesystem_results f JOIN filesystem_execution e ON e.request_id=f.request_id WHERE e.request_json LIKE ? AND json_extract(e.request_json,'$.operation.operation.action')=?").all(`%${id}%`, actionFor(stage));
  } finally { database.close(); }
  assert.equal(rows.length, 1, 'Exactly one original remote mutation, including failures');
  const row = rows[0], request = JSON.parse(row.request_json), context = JSON.parse(row.context_json), reply = JSON.parse(row.reply_json);
  assert.equal(row.request_id, request.request_id); assert.equal(reply.request_id, row.request_id);
  assert.equal(reply.state, 'completed'); assert.equal(reply.file_manager.request_id, row.request_id); assert.equal(reply.file_manager.result.result, 'applied');
  return { request_id: row.request_id, operation: request.operation.operation, state: 'completed', result: 'applied', execution_identity: context.identity, reply_identity: reply.file_manager.execution_context.identity };
}
const stageReport = stage => stage === 'create' && fs.existsSync(path.join(base, 'ui-recover-create.json')) ? path.join(base, 'ui-recover-create.json') : path.join(base, `ui-${stage}.json`);
if (phase === 'prepare') {
  assert(!fs.existsSync(base)); fs.mkdirSync(base); fs.mkdirSync(folder);
  for (const [name, text] of [['seed.txt', 'Native file-operation seed\n'], ['zeta-中文.txt', 'Native unicode seed\n'], ['hidden.txt', 'Native hidden seed\n']]) fs.writeFileSync(path.join(folder, name), text, { flag: 'wx' });
  execFileSync('C:\\Windows\\System32\\attrib.exe', ['+h', path.join(folder, 'hidden.txt')], { timeout: 10000 });
  const prepared = { schema: 1, run_id: id, root, folder, host: os.hostname(), side: options.get('--side'), actor, executor_sha256: marker.executor_sha256, desktop_sha256: marker.desktop_sha256, seeds: fs.readdirSync(folder).sort().map(name => ({ name, bytes: fs.statSync(path.join(folder, name)).size, sha256: shaFile(path.join(folder, name)) })) };
  assert(['local', 'remote'].includes(prepared.side)); writeJson(path.join(base, 'prepared.json'), prepared);
  console.log(JSON.stringify({ phase, run_id: id, folder, actor, prepared: path.join(base, 'prepared.json') }));
  process.exit(0);
}
const preparedLocal = readJson(path.join(base, 'prepared.json')); assert.equal(preparedLocal.run_id, id); seeds(preparedLocal);
if (phase === 'observe') {
  assert.equal(preparedLocal.side, 'remote');
  const stage = options.get('--stage'); assert(['create', 'rename', 'delete'].includes(stage));
  const reportFile = contained(root, options.get('--ui-report')), report = readJson(reportFile);
  assert(report.run_id === id && report.phase === stage && report.status === 'awaiting_independent_target' && report.target_fixture === root && report.host !== os.hostname());
  assert.equal(report.target_executor_sha256, marker.executor_sha256);
  const mutation = remoteMutation(stage);
  assert.equal(mutation.execution_identity.account_id, actor.sid);
  assert(mutation.execution_identity.mode === 'user' && mutation.execution_identity.session_id === String(actor.session));
  assert.equal(mutation.execution_identity.logon_id, report.remote_identity.logon_id);
  assert.equal(mutation.execution_identity.account_name, actor.name);
  assert.deepEqual(mutation.reply_identity, mutation.execution_identity);
  const exists = name => fs.existsSync(path.join(folder, name));
  assert(!exists(cancelledName));
  if (stage === 'create') assert(exists(createdName) && !exists(renamedName));
  if (stage === 'rename') assert(!exists(createdName) && exists(renamedName));
  if (stage === 'delete') assert(!exists(createdName) && !exists(renamedName));
  const identity = stage === 'delete' ? null : directoryIdentity(path.join(folder, stage === 'create' ? createdName : renamedName));
  if (stage !== 'delete') assert.equal(fs.readdirSync(path.join(folder, stage === 'create' ? createdName : renamedName)).length, 0);
  if (stage === 'rename') assert.deepEqual(identity, readJson(path.join(base, 'observed-create.json')).directory_identity, 'Rename must preserve the actual directory object');
  const observation = { pass: true, run_id: id, phase: stage, host: os.hostname(), actor, directory_identity: identity, report_sha256: shaFile(reportFile), original_request_id: mutation.request_id, original_mutation: mutation, prepared_sha256: shaFile(path.join(base, 'prepared.json')), checks: { seed_bytes_preserved: true, original_mutation_acknowledged: true, actual_filesystem_matches: true, cancelled_name_absent: true } };
  writeJson(path.join(base, `observed-${stage}.json`), observation); console.log(JSON.stringify(observation)); process.exit(0);
}
assert.equal(preparedLocal.side, 'local');
const sourceCase = options.get('--source-case'), original = runDirectory(root, sourceCase);
const offer = readJson(path.join(original, 'offer.json')), target = readJson(path.join(original, 'transport', 'target-prepared.json'));
assert.equal(target.fixture, options.get('--target-fixture')); assert.equal(offer.target_fixture, target.fixture);
const targetFolder = path.join(target.fixture, 'payload', `native-ops-${id}`, 'files');
const output = path.join(base, `ui-${phase}.json`); assert(!fs.existsSync(output), 'Inspect original mutation IDs; never replay a phase to replace its outcome');
const report = { schema: 1, run_id: id, phase, pass: false, status: 'started', host: os.hostname(), actor, source_case: sourceCase, target_fixture: target.fixture, target_code: target.code, desktop_sha256: marker.desktop_sha256, target_executor_sha256: target.executor_sha256, driver_sha256: shaFile(fileURLToPath(import.meta.url)), checks: {}, mutations: [] };
if (phase === 'recover-create') {
  // Read-only reconciliation of the original UI run, never a resubmission.
  const originalFile = path.join(base, 'ui-create.json'), previous = readJson(originalFile);
  assert(previous.status === 'failed' && previous.phase === 'create' && previous.run_id === id && previous.error?.name === 'AssertionError');
  assert(previous.checks.local_create_cancel_properties_navigation_search_hidden && previous.checks.remote_create_cancel_properties_navigation_search_hidden);
  assert(previous.target_fixture === target.fixture && previous.desktop_sha256 === marker.desktop_sha256);
  assert(fs.statSync(path.join(folder, createdName)).isDirectory() && !fs.existsSync(path.join(folder, cancelledName)));
  report.phase = 'create'; report.checks = previous.checks; report.remote_identity = previous.remote_identity; report.link = previous.link;
  report.original_failed_ui = { file: originalFile, sha256: shaFile(originalFile), driver_sha256: previous.driver_sha256 };
  report.local_directory_identity = directoryIdentity(path.join(folder, createdName));
  report.mutations = [localMutation('create')]; report.status = 'awaiting_independent_target';
  writeJson(output, report); console.log(JSON.stringify(report)); process.exit(0);
}
const runtime = options.get('--browser-runtime'); assert(runtime && path.isAbsolute(runtime));
const { chromium } = createRequire(import.meta.url)(path.join(runtime, 'node_modules', 'playwright-core'));
const invoke = (page, command, values = {}) => page.evaluate(({ command, values }) => window.__TAURI_INTERNALS__.invoke(command, values), { command, values });
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function until(action, predicate, message) {
  const deadline = performance.now() + 30000;
  while (performance.now() < deadline) { const result = await action(); if (predicate(result)) return result; await wait(100); }
  throw new Error(`Timeout: ${message}`);
}
let browser;
try {
  const desktop = readJson(path.join(root, 'desktop-process.json'));
  assert.equal(shaFile(contained(root, desktop.exe)), marker.desktop_sha256);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${desktop.cdp}`);
  const main = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().includes('fileManager=1')); assert(main);
  const account = await invoke(main, 'operator_current_traffic_scope');
  assert.equal(account?.userId, readJson(path.join(root, 'owned-test-account.json')).user_id);
  await main.evaluate(() => localStorage.setItem('pab.language', 'en'));
  if (phase === 'create') {
    const access = unseal(path.join(original, 'transport', 'controller-private.pem'), readJson(path.join(original, 'transport', 'access-envelope.json')), sourceCase);
    assert(access.code === target.code && access.fixture === target.fixture && access.executor_sha256 === target.executor_sha256);
    await invoke(main, 'operator_connect', { code: access.code, password: access.password }); access.password = undefined;
  }
  const windowLabel = await invoke(main, 'file_manager_window', { code: target.code });
  const files = await until(async () => browser.contexts().flatMap(context => context.pages()).find(page => page.url().includes('fileManager=1')), Boolean, 'native file window');
  await until(() => files.locator('.fm-pane').count(), count => count === 2, 'both native panes');
  assert.equal(await invoke(main, 'file_manager_window', { code: target.code }), windowLabel);
  assert((await invoke(files, 'file_manager_batches')).every(batch => !batch.runner_active));
  await files.getByRole('checkbox', { name: 'Force Relay', exact: true }).setChecked(true);
  const inventory = await invoke(files, 'file_manager_contexts', { code: target.code, forceRelay: true });
  assert.equal(inventory.state, 'completed');
  const users = inventory.data.entries.filter(entry => entry.mode === 'user' && entry.selection && entry.account_id === offer.remote_user.account_id && entry.account_name === offer.remote_user.account_name && entry.identity?.session_id === offer.remote_user.session_id && entry.identity?.logon_id === offer.remote_user.logon_id);
  assert.equal(users.length, 1, 'Original native identity/session required');
  report.remote_identity = users[0].identity;
  await files.getByRole('combobox', { name: 'Remote account', exact: true }).selectOption({ label: `${users[0].account_name} · ${users[0].session_id}` });
  report.link = await invoke(files, 'file_manager_link_status', { code: target.code, forceRelay: true }); assert(report.link.connected && report.link.path === 'relay');
  if (phase === 'finish') {
    const ids = new Set();
    for (const stage of ['create', 'rename', 'delete']) {
      const localFile = stageReport(stage), local = readJson(localFile), remoteFile = path.join(base, `observed-${stage}.json`), remote = readJson(remoteFile);
      assert(local.status === 'awaiting_independent_target' && remote.pass && remote.run_id === id && remote.report_sha256 === shaFile(localFile));
      for (const row of local.mutations) { assert(!ids.has(row.request_id)); ids.add(row.request_id); }
      assert(!ids.has(remote.original_request_id)); ids.add(remote.original_request_id);
    }
    seeds(preparedLocal); assert(!fs.existsSync(path.join(folder, renamedName)) && !fs.existsSync(path.join(folder, createdName)));
    report.pass = true; report.status = 'native_file_operations_passed'; report.checks.independent_target_all_phases = true;
    report.original_mutation_ids = [...ids]; assert.equal(ids.size, 6);
    await files.screenshot({ path: path.join(base, 'finished.png') });
  } else {
    // Native journals are authoritative. WebView invoke properties may be
    // immutable; monkey-patching them is not evidence of sent request IDs.
    const panes = [{ pane: files.locator('.fm-pane').first(), folder }, { pane: files.locator('.fm-pane').last(), folder: targetFolder }];
    for (let index = 0; index < panes.length; index++) {
      const { pane, folder: currentFolder } = panes[index], prefix = index ? 'remote' : 'local';
      const navigate = async destination => { const input = pane.locator('.fm-pathbar input'); await input.fill(destination); await input.press('Enter'); await until(async () => ({ value: await input.inputValue(), status: await pane.locator('.fm-pane-status [role="status"]').innerText(), errors: await pane.locator('.fm-error').count() }), value => value.value === destination && /\d+ items/.test(value.status) && !value.errors, 'owned folder'); };
      await navigate(currentFolder);
      for (const selected of await pane.locator('.fm-file-row input:checked').all()) await selected.uncheck();
      const dialog = name => files.getByRole('dialog', { name, exact: true });
      if (phase === 'create') {
        await pane.getByTitle('New folder', { exact: true }).click(); await dialog('New folder').getByLabel('Name', { exact: true }).fill(cancelledName); await dialog('New folder').locator('footer').getByRole('button', { name: 'Cancel', exact: true }).click();
        await pane.getByTitle('New folder', { exact: true }).click(); await dialog('New folder').getByLabel('Name', { exact: true }).fill(createdName); await dialog('New folder').locator('footer').getByRole('button', { name: 'Confirm', exact: true }).click(); await dialog('New folder').waitFor({ state: 'hidden' });
        await pane.getByRole('checkbox', { name: `Select ${createdName}`, exact: true }).waitFor();
        await pane.getByRole('checkbox', { name: 'Select seed.txt', exact: true }).check(); await pane.getByTitle('Properties', { exact: true }).click();
        const details = await dialog('Properties').locator('dd').allTextContents(); assert(details.includes('seed.txt') && details.includes(path.join(currentFolder, 'seed.txt')) && details.includes(`${Buffer.byteLength('Native file-operation seed\n')} B`));
        await dialog('Properties').locator('footer').getByRole('button', { name: 'Close', exact: true }).click(); await pane.getByRole('checkbox', { name: 'Select seed.txt', exact: true }).uncheck();
        await pane.locator('.fm-file-row').filter({ hasText: createdName }).dblclick(); await until(() => pane.locator('.fm-pathbar input').inputValue(), value => value === path.join(currentFolder, createdName), 'child navigation');
        await pane.getByTitle('Back', { exact: true }).click(); await until(() => pane.locator('.fm-pathbar input').inputValue(), value => value === currentFolder, 'back navigation');
        await pane.getByTitle('Forward', { exact: true }).click(); await until(() => pane.locator('.fm-pathbar input').inputValue(), value => value === path.join(currentFolder, createdName), 'forward navigation');
        await pane.getByTitle('Parent folder', { exact: true }).click(); await until(() => pane.getByRole('checkbox', { name: `Select ${createdName}`, exact: true }).isEnabled(), Boolean, 'parent navigation');
        await pane.locator('.fm-search input').fill('seed.txt'); await until(() => pane.locator('.fm-file-row').count(), count => count === 1, 'search result'); assert.equal(await pane.locator('.fm-file-row .fm-file-name > span').innerText(), 'seed.txt'); await pane.locator('.fm-search input').fill('');
        await pane.locator('.fm-hidden input').check(); await pane.getByRole('checkbox', { name: 'Select hidden.txt', exact: true }).waitFor(); await pane.locator('.fm-hidden input').uncheck(); await pane.getByRole('checkbox', { name: 'Select hidden.txt', exact: true }).waitFor({ state: 'hidden' });
        report.checks[`${prefix}_create_cancel_properties_navigation_search_hidden`] = true;
      } else {
        const name = phase === 'rename' ? createdName : renamedName; await pane.getByRole('checkbox', { name: `Select ${name}`, exact: true }).check();
        const title = phase === 'rename' ? 'Rename' : 'Delete'; await pane.getByTitle(title, { exact: true }).click();
        if (phase === 'rename') await dialog(title).getByLabel('Name', { exact: true }).fill(cancelledName);
        await dialog(title).locator('footer').getByRole('button', { name: 'Cancel', exact: true }).click(); assert(await pane.getByRole('checkbox', { name: `Select ${name}`, exact: true }).isChecked());
        await pane.getByTitle(title, { exact: true }).click(); if (phase === 'rename') await dialog(title).getByLabel('Name', { exact: true }).fill(renamedName);
        await dialog(title).locator('footer').getByRole('button', { name: 'Confirm', exact: true }).click(); await dialog(title).waitFor({ state: 'hidden' });
        await pane.getByRole('checkbox', { name: `Select ${name}`, exact: true }).waitFor({ state: 'hidden' });
        if (phase === 'rename') await pane.getByRole('checkbox', { name: `Select ${renamedName}`, exact: true }).waitFor();
        report.checks[`${prefix}_${phase}_and_cancel`] = true;
      }
    }
    report.mutations = [localMutation(phase)];
    // Target-side read-only observation must add its original request ID and
    // verify exactly one native request, real user context and physical result.
    seeds(preparedLocal);
    if (phase === 'create') assert(fs.statSync(path.join(folder, createdName)).isDirectory());
    if (phase === 'rename') assert(!fs.existsSync(path.join(folder, createdName)) && fs.statSync(path.join(folder, renamedName)).isDirectory());
    if (phase === 'delete') assert(!fs.existsSync(path.join(folder, createdName)) && !fs.existsSync(path.join(folder, renamedName)));
    if (phase !== 'delete') report.local_directory_identity = directoryIdentity(path.join(folder, phase === 'create' ? createdName : renamedName));
    if (phase === 'rename') assert.deepEqual(report.local_directory_identity, readJson(stageReport('create')).local_directory_identity, 'Local rename must preserve its original directory object');
    report.status = 'awaiting_independent_target';
  }
  writeJson(output, report); console.log(JSON.stringify(report));
} catch (error) {
  report.status = 'failed'; report.error = { name: error.name, message: error.message };
  if (!fs.existsSync(output)) writeJson(output, report);
  throw error;
} finally { if (browser) await browser.close(); }
