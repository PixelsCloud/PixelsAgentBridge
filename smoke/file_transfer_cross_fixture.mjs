// Native cross-host test fixtures and independent receiver observations.
// All paths belong to an explicitly prepared isolated candidate root. Account
// access is read only on its owning computer and handed off encrypted. Use
// Pixels file transfer for offers, public keys, envelopes and observations.
// This bounded adapter currently validates Windows pairs, not other platforms.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { createHash, randomBytes, generateKeyPairSync, publicEncrypt, privateDecrypt, constants, createCipheriv, createDecipheriv } from 'node:crypto';

export const shaFile = filename => createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
export function readJson(filename) {
  assert(fs.statSync(filename).size <= 1024 * 1024, 'Oversized fixture metadata');
  return JSON.parse(fs.readFileSync(filename, 'utf8').replace(/^\uFEFF/, ''));
}
export function writeJson(filename, value) {
  fs.writeFileSync(filename, JSON.stringify(value, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
}
export function contained(root, filename, exists = true) {
  const resolved = exists ? fs.realpathSync(filename) : path.resolve(filename);
  const relative = path.relative(root, resolved);
  assert(relative && !relative.startsWith('..') && !path.isAbsolute(relative), 'Path escaped owned fixture');
  return resolved;
}
export function fixtureRoot(value) {
  assert(value && path.isAbsolute(value), 'An absolute owned fixture is required');
  const root = fs.realpathSync(value);
  assert(fs.statSync(root).isDirectory());
  const record = ['desktop-process.json', 'service-process.json'].map(name => path.join(root, name)).find(name => fs.existsSync(name));
  assert(record, 'Owned candidate launch record is required');
  const marker = readJson(record); contained(root, marker.exe); contained(root, marker.config);
  return root;
}
export function runDirectory(root, id) {
  assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id), 'Explicit v4 run UUID required');
  return contained(root, path.join(root, 'payload', `cross-${id}`), false);
}
export function manifest(base) {
  const rows = [];
  function walk(relative) {
    for (const name of fs.readdirSync(path.join(base, relative)).sort()) {
      const child = path.join(relative, name), full = path.join(base, child), stat = fs.lstatSync(full);
      assert(!stat.isSymbolicLink(), 'Unexpected fixture link');
      const modified_at_unix_ms = Math.floor(stat.mtimeMs);
      if (stat.isDirectory()) { rows.push({ path: child, kind: 'directory', modified_at_unix_ms }); walk(child); }
      else { assert(stat.isFile() && stat.size <= 32 * 1024 * 1024, 'Not a bounded ordinary fixture file'); rows.push({ path: child, kind: 'file', bytes: stat.size, sha256: shaFile(full), modified_at_unix_ms }); }
      assert(rows.length <= 1000, 'Not a bounded native fixture');
    }
  }
  walk(''); return rows;
}
export function assertManifest(actual, expected, description) {
  const content = rows => rows.map(({ modified_at_unix_ms, ...entry }) => entry);
  assert.deepEqual(content(actual), content(expected), `${description}: names/types/sizes/SHA differ`);
  // These native Windows fixtures use the protocol's millisecond timestamp
  // contract; allow at most two milliseconds for OS conversion/rounding.
  for (let i = 0; i < actual.length; i++) assert(Math.abs(actual[i].modified_at_unix_ms - expected[i].modified_at_unix_ms) <= 2, `${description}: modification time differs`);
}
function fixtureBytes(value = 8 * 1024 * 1024) {
  assert(Number.isSafeInteger(value) && value >= 4096 && value <= 32 * 1024 * 1024, 'Fixture data must be 4 KiB..32 MiB');
  return value;
}
function createTree(base, name, label, bytes = 8 * 1024 * 1024) {
  fs.mkdirSync(path.join(base, name, 'nested'), { recursive: true });
  fs.mkdirSync(path.join(base, name, 'empty'));
  fs.writeFileSync(path.join(base, name, 'zero.bin'), Buffer.alloc(0), { flag: 'wx' });
  fs.writeFileSync(path.join(base, name, 'nested', 'random.bin'), randomBytes(fixtureBytes(bytes)), { flag: 'wx' });
  fs.writeFileSync(path.join(base, name, 'nested', 'unicode-空格 name.txt'), `Independent ${label} synthetic fixture\n`, { flag: 'wx' });
}
export function prepareController(root, id, targetMachine, targetRoot, executorHash, mode, execution, remoteUser = null, bytes = 8 * 1024 * 1024) {
  assert.equal(process.platform, 'win32', 'This native pair adapter requires Windows');
  assert(targetMachine && targetMachine !== os.hostname(), 'A different explicit target machine is required');
  assert(path.isAbsolute(targetRoot) && /^[a-f0-9]{64}$/.test(executorHash));
  assert(['auto', 'relay'].includes(mode) && ['service', 'user'].includes(execution));
  if (execution === 'user') assert(remoteUser?.account_name && remoteUser.account_id && remoteUser.session_id && remoteUser.logon_id && remoteUser.token_owner_sid, 'Explicit sampled target native user/token owner required');
  fixtureBytes(bytes);
  const directory = runDirectory(root, id);
  fs.mkdirSync(directory); for (const name of ['source', 'download', 'transport']) fs.mkdirSync(path.join(directory, name));
  createTree(path.join(directory, 'source'), 'controller-folder-中文', 'controller', bytes);
  const keys = generateKeyPairSync('rsa', { modulusLength: 4096, publicKeyEncoding: { type: 'spki', format: 'pem' }, privateKeyEncoding: { type: 'pkcs8', format: 'pem' } });
  fs.writeFileSync(path.join(directory, 'transport', 'controller-private.pem'), keys.privateKey, { flag: 'wx', mode: 0o600 });
  fs.writeFileSync(path.join(directory, 'transport', 'controller-public.pem'), keys.publicKey, { flag: 'wx' });
  const offer = { schema: 1, run_id: id, controller_machine: os.hostname(), controller_fixture: root, target_machine: targetMachine, target_fixture: targetRoot, target_executor_sha256: executorHash, connection_mode: mode, execution, remote_user: remoteUser, tree_name: 'controller-folder-中文', expected_manifest: manifest(path.join(directory, 'source')), public_key_sha256: shaFile(path.join(directory, 'transport', 'controller-public.pem')) };
  offer.fixture_bytes = bytes;
  writeJson(path.join(directory, 'offer.json'), offer); return { directory, offer };
}
function seal(publicKey, payload, id) {
  const key = randomBytes(32), iv = randomBytes(12), plain = Buffer.from(JSON.stringify(payload));
  try {
    const cipher = createCipheriv('aes-256-gcm', key, iv); cipher.setAAD(Buffer.from(`pab-ft-cross-v1\n${id}`));
    const ciphertext = Buffer.concat([cipher.update(plain), cipher.final()]);
    return { schema: 1, run_id: id, algorithm: 'RSA-OAEP-SHA256/AES-256-GCM', wrapped_key: publicEncrypt({ key: publicKey, padding: constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256' }, key).toString('base64'), iv: iv.toString('base64'), tag: cipher.getAuthTag().toString('base64'), ciphertext: ciphertext.toString('base64') };
  } finally { key.fill(0); plain.fill(0); }
}
export function unseal(privateKeyFile, envelope, id) {
  assert(envelope.schema === 1 && envelope.run_id === id && envelope.algorithm === 'RSA-OAEP-SHA256/AES-256-GCM');
  const key = privateDecrypt({ key: fs.readFileSync(privateKeyFile), padding: constants.RSA_PKCS1_OAEP_PADDING, oaepHash: 'sha256' }, Buffer.from(envelope.wrapped_key, 'base64'));
  let plain;
  try {
    assert.equal(key.length, 32);
    const cipher = createDecipheriv('aes-256-gcm', key, Buffer.from(envelope.iv, 'base64')); cipher.setAAD(Buffer.from(`pab-ft-cross-v1\n${id}`)); cipher.setAuthTag(Buffer.from(envelope.tag, 'base64'));
    plain = Buffer.concat([cipher.update(Buffer.from(envelope.ciphertext, 'base64')), cipher.final()]);
    assert(plain.length <= 8192); return JSON.parse(plain.toString('utf8'));
  } finally { key.fill(0); plain?.fill(0); }
}
function ownAccess(root) {
  const marker = readJson(path.join(root, 'service-process.json'));
  const executor = contained(root, marker.exe), config = contained(root, marker.config);
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('PAB_')));
  let buffer;
  try { buffer = execFileSync(executor, ['--config', config, 'show-access'], { env, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], timeout: 10000 }); }
  catch { throw new Error('Owned candidate access is not ready; no access output is logged'); }
  try {
    const text = buffer.toString('utf8');
    const code = text.match(/Device code:\s*(\d{9})/)?.[1], password = text.match(/Temporary password:\s*(\S+)/)?.[1];
    assert(code && password, 'Owned access could not be parsed'); return { code, password, executor_sha256: shaFile(executor) };
  } finally { buffer?.fill(0); }
}
export function prepareTarget(root, offerFile, publicFile) {
  assert.equal(process.platform, 'win32', 'This native pair adapter requires Windows');
  const offer = readJson(contained(root, offerFile));
  assert(offer.schema === 1 && offer.target_machine === os.hostname() && offer.target_machine !== offer.controller_machine);
  assert.equal(path.resolve(offer.target_fixture), root);
  assert.equal(shaFile(contained(root, publicFile)), offer.public_key_sha256);
  const bytes = fixtureBytes(offer.fixture_bytes);
  const access = ownAccess(root); assert.equal(access.executor_sha256, offer.target_executor_sha256);
  const directory = runDirectory(root, offer.run_id); fs.mkdirSync(directory);
  for (const name of ['receive', 'source']) fs.mkdirSync(path.join(directory, name));
  createTree(path.join(directory, 'source'), 'target-folder-中文', 'target', bytes);
  const prepared = { schema: 1, run_id: offer.run_id, host: os.hostname(), platform: process.platform, release: os.release(), process_user: os.userInfo().username, fixture: root, code: access.code, executor_sha256: access.executor_sha256, receive: path.join(directory, 'receive'), source: path.join(directory, 'source'), tree_name: 'target-folder-中文', source_manifest: manifest(path.join(directory, 'source')), offer_sha256: shaFile(offerFile), collector_sha256: shaFile(fileURLToPath(import.meta.url)) };
  if (offer.execution === 'user') {
    assert(/^S-1-\d+(?:-\d+)+$/.test(offer.remote_user.account_id));
    // Only this new receive directory changes ACL. The selected user can write;
    // the SYSTEM parent cannot silently perform the same mutation instead.
    const icacls = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'icacls.exe');
    execFileSync(icacls, [prepared.receive, '/inheritance:r', '/grant:r', `*${offer.remote_user.account_id}:(OI)(CI)F`, '/deny', '*S-1-5-18:(OI)(CI)M'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000 });
    const probe = path.join(prepared.receive, 'service-write-probe');
    let denied = false;
    try { fs.writeFileSync(probe, Buffer.from([1]), { flag: 'wx' }); }
    catch (error) { if (['EACCES', 'EPERM'].includes(error.code)) denied = true; else throw error; }
    if (!denied) { fs.unlinkSync(probe); throw new Error('The owned receive ACL did not reject SYSTEM writes'); }
    prepared.user_boundary = { deny_system_modify: true, system_write_denied: true, allowed_user_sid: offer.remote_user.account_id, expected_token_owner_sid: offer.remote_user.token_owner_sid };
  }
  const envelope = seal(fs.readFileSync(publicFile), { schema: 1, run_id: offer.run_id, code: access.code, password: access.password, host: os.hostname(), fixture: root, executor_sha256: access.executor_sha256 }, offer.run_id);
  access.password = undefined;
  writeJson(path.join(directory, 'target-prepared.json'), prepared);
  writeJson(path.join(directory, 'access-envelope.json'), envelope);
  return { directory, prepared: path.join(directory, 'target-prepared.json'), envelope: path.join(directory, 'access-envelope.json'), code: prepared.code, host: prepared.host };
}
export function observeTarget(root, offerFile) {
  assert.equal(process.platform, 'win32', 'This native pair adapter requires Windows');
  const offer = readJson(contained(root, offerFile)), directory = contained(root, runDirectory(root, offer.run_id));
  const prepared = readJson(path.join(directory, 'target-prepared.json'));
  assert.equal(prepared.host, os.hostname()); assert.equal(prepared.offer_sha256, shaFile(offerFile));
  const received = manifest(contained(root, prepared.receive)), source = manifest(contained(root, prepared.source));
  assertManifest(received, offer.expected_manifest, 'Independent target receive manifest');
  assertManifest(source, prepared.source_manifest, 'Target-origin source');
  const owners = windowsOwners(prepared.receive, received);
  if (offer.execution === 'user') assert(owners.length === received.length && owners.every(entry => entry.owner_sid === offer.remote_user.token_owner_sid), 'Receiver files/directories were not created with the explicitly selected native token owner');
  const result = { schema: 1, run_id: offer.run_id, host: os.hostname(), process_user: os.userInfo().username, platform: process.platform, release: os.release(), pass: true, code: prepared.code, received_manifest: received, source_manifest: source, receive_owners: owners, selected_token_owner_verified: offer.execution === 'user', timestamp_tolerance_ms: 2, offer_sha256: shaFile(offerFile), prepared_sha256: shaFile(path.join(directory, 'target-prepared.json')), observer_sha256: shaFile(fileURLToPath(import.meta.url)), observed: new Date().toISOString() };
  const output = path.join(directory, 'target-observed.json'); writeJson(output, result); return { report: output, sha256: shaFile(output), pass: true };
}
export function restoreTargetBoundary(root, offerFile, cancelledObservation, cancelledControl) {
  const offer = readJson(contained(root, offerFile)), directory = contained(root, runDirectory(root, offer.run_id));
  const prepared = readJson(path.join(directory, 'target-prepared.json'));
  assert.equal(prepared.host, os.hostname()); assert.equal(prepared.offer_sha256, shaFile(offerFile));
  assert.equal(prepared.receive, path.join(directory, 'receive'));
  assert.equal(!!cancelledObservation, !!cancelledControl, 'Partial restoration requires both independent observation and original control report');
  let expected = offer.expected_manifest;
  if (cancelledObservation) {
    const observation = readJson(contained(root, cancelledObservation)), controlFile = contained(root, cancelledControl), control = readJson(controlFile);
    assert(observation.pass && observation.phase === 'cancelled' && observation.run_id === offer.run_id && observation.offer_sha256 === shaFile(offerFile));
    assert.equal(observation.control_sha256, shaFile(controlFile));
    assert(control.phase === 'cancel-upload' && control.status === 'cancelled_waiting_independent_target' && control.run_id === offer.run_id && control.offer_sha256 === shaFile(offerFile));
    assert.equal(control.prepared_sha256, shaFile(path.join(directory, 'target-prepared.json')));
    assert(control.cancelled.batch.phase === 'cancelled' && !control.cancelled.batch.runner_active && control.active_item.state === 'cancelled');
    const { DatabaseSync } = createRequire(import.meta.url)('node:sqlite');
    const database = contained(root, path.join(root, 'executor-data', 'executor.sqlite3')), db = new DatabaseSync(database, { readOnly: true });
    try {
      const row = db.prepare('SELECT state,direction,offset,finished_at_unix_ms FROM transfer_operations WHERE request_id=?').get(control.active_item.attempt);
      const execution = db.prepare('SELECT context_json FROM transfer_execution WHERE request_id=?').get(control.active_item.attempt);
      assert(row && row.state === 'cancelled' && row.direction === 'receive' && row.finished_at_unix_ms != null && row.offset === Number(control.active_item.confirmed));
      assert(execution); assert.deepEqual(JSON.parse(execution.context_json).identity, control.selected_identity);
    } finally { db.close(); }
    assert(Array.isArray(observation.partial_manifest)); expected = observation.partial_manifest;
  }
  assertManifest(manifest(prepared.receive), expected, 'Owned receive before ACL restoration');
  assertManifest(manifest(prepared.source), prepared.source_manifest, 'Original source before ACL restoration');
  if (offer.execution === 'user') {
    const executable = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'icacls.exe');
    execFileSync(executable, [prepared.receive, '/reset', '/T', '/C'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000 });
  }
  const result = { run_id: offer.run_id, scope: 'Restored inherited ACL on only the verified owned receive subtree after original batches completed', restored: true, host: os.hostname(), at: new Date().toISOString() };
  const output = path.join(directory, 'target-acl-restored.json'); writeJson(output, result); return { report: output, restored: true };
}
function windowsOwners(root, entries) {
  const input = Buffer.from(JSON.stringify({ root, paths: entries.map(entry => entry.path) })).toString('base64');
  const script = "$ErrorActionPreference='Stop';[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false);$inputData=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($env:PAB_SMOKE_OWNER_INPUT))|ConvertFrom-Json;$root=[IO.Path]::GetFullPath([string]$inputData.root);$rows=@(foreach($relative in $inputData.paths){$full=[IO.Path]::GetFullPath((Join-Path $root ([string]$relative)));if(-not $full.StartsWith($root+'\\',[StringComparison]::OrdinalIgnoreCase)){throw 'Owner query escaped fixture'};[ordered]@{path=[string]$relative;owner_sid=[string](Get-Acl -LiteralPath $full).GetOwner([Security.Principal.SecurityIdentifier]).Value}});ConvertTo-Json -InputObject $rows -Depth 3 -Compress";
  const executable = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const buffer = execFileSync(executable, ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], { env: { ...process.env, PAB_SMOKE_OWNER_INPUT: input }, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], timeout: 15000 });
  return JSON.parse(buffer.toString('utf8').replace(/^\uFEFF/, ''));
}
if (process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url))) {
  try {
    const args = new Map();
    for (let i = 2; i < process.argv.length; i += 2) { assert(['--phase', '--fixture', '--offer', '--public-key', '--cancelled-observation', '--cancelled-control'].includes(process.argv[i]) && process.argv[i + 1] && !args.has(process.argv[i])); args.set(process.argv[i], process.argv[i + 1]); }
    const root = fixtureRoot(args.get('--fixture'));
    const result = args.get('--phase') === 'prepare-target' ? prepareTarget(root, args.get('--offer'), args.get('--public-key')) : args.get('--phase') === 'observe-target' ? observeTarget(root, args.get('--offer')) : args.get('--phase') === 'restore-target' ? restoreTargetBoundary(root, args.get('--offer'), args.get('--cancelled-observation'), args.get('--cancelled-control')) : assert.fail('Explicit prepare-target, observe-target or restore-target phase required');
    console.log(JSON.stringify(result));
  } catch (error) { console.error(JSON.stringify({ pass: false, error: String(error.message || error) })); process.exitCode = 1; }
}
