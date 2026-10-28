// Independent on-target observations of the original native transfer IDs.
// Uses Node's built-in SQLite API in read-only mode against ONLY the explicitly
// prepared candidate database. No task submission, SQL mutation or cleanup.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { DatabaseSync } from 'node:sqlite';
import { fixtureRoot, runDirectory, contained, readJson, writeJson, manifest, assertManifest, shaFile } from './file_transfer_cross_fixture.mjs';

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index], value = process.argv[index + 1];
  assert(['--phase', '--fixture', '--offer', '--control', '--observation-id'].includes(key) && value && !args.has(key)); args.set(key, value);
}
const phase = args.get('--phase'); assert(['pause', 'completed', 'cancelled'].includes(phase));
assert.equal(process.platform, 'win32');
const root = fixtureRoot(args.get('--fixture')), offerFile = contained(root, args.get('--offer')), offer = readJson(offerFile);
const directory = runDirectory(root, offer.run_id), preparedFile = contained(root, path.join(directory, 'target-prepared.json')), prepared = readJson(preparedFile);
const observationId = args.get('--observation-id');
if (observationId) assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(observationId));
const output = path.join(directory, `target-control-${phase}${observationId ? `-${observationId}` : ''}.json`);
assert(!fs.existsSync(output), 'Keep the original observation; use an explicit new observation UUID');
const controlFile = contained(root, args.get('--control')), control = readJson(controlFile);
assert.equal(offer.target_machine, os.hostname()); assert.equal(prepared.host, os.hostname());
assert.equal(control.run_id, offer.run_id); assert.equal(control.offer_sha256, shaFile(offerFile)); assert.equal(control.prepared_sha256, shaFile(preparedFile));
assert(offer.execution === 'user' && prepared.user_boundary.system_write_denied);
const pauseDirection = control.pause_direction || 'upload', cancelDirection = control.cancel_direction || 'download';
assert(['upload', 'download'].includes(pauseDirection) && ['upload', 'download'].includes(cancelDirection) && pauseDirection !== cancelDirection);
const direction = phase === 'cancelled' ? cancelDirection : pauseDirection, receiver = direction === 'upload';
assert.equal(control.phase, { pause: `pause-${pauseDirection}`, completed: `resume-${pauseDirection}`, cancelled: `cancel-${cancelDirection}` }[phase]);
assert.equal(control.status, { pause: 'paused_waiting_independent_target', completed: `${pauseDirection}_complete_waiting_independent_target`, cancelled: 'cancelled_waiting_independent_target' }[phase]);
const active = control.active_item; assert(active?.attempt && /^[0-9a-f-]{36}$/.test(active.attempt));
assert(BigInt(active.spec.size) === 32n * 1024n * 1024n);
const source = contained(root, prepared.source), receive = contained(root, prepared.receive);
const database = contained(root, path.join(root, 'executor-data', 'executor.sqlite3'));
assert(fs.lstatSync(database).isFile() && !fs.lstatSync(database).isSymbolicLink());
const db = new DatabaseSync(database, { readOnly: true });
const report = { schema: 1, run_id: offer.run_id, phase, pass: false, host: os.hostname(), process_user: os.userInfo().username, scope: 'Independent read-only on-target original transfer row, frozen native execution, checkpoint and final disk observations', offer_sha256: shaFile(offerFile), control_sha256: shaFile(controlFile), observer_sha256: shaFile(fileURLToPath(import.meta.url)), started: new Date().toISOString() };
function originalRow() {
  const row = db.prepare('SELECT request_id,direction,path,size,offset,state,started_at_unix_ms,finished_at_unix_ms,sha256 FROM transfer_operations WHERE request_id=?').get(active.attempt);
  const execution = db.prepare('SELECT request_json,context_json FROM transfer_execution WHERE request_id=?').get(active.attempt);
  assert(row && execution, 'Original on-target transfer/execution facts are missing');
  const context = JSON.parse(execution.context_json), request = JSON.parse(execution.request_json);
  assert.equal(row.request_id, active.attempt); assert.equal(request.request_id, active.attempt);
  assert.equal(context.identity.mode, 'user');
  for (const key of ['mode', 'account_id', 'account_name', 'home', 'primary_group', 'session_id', 'logon_id', 'environment_source']) assert.deepEqual(context.identity[key] ?? null, control.selected_identity[key] ?? null, `Frozen target identity differs: ${key}`);
  assert.equal(context.identity.account_id, offer.remote_user.account_id);
  assert.equal(context.identity.logon_id, offer.remote_user.logon_id);
  const expected = receiver ? (active.effective_destination || active.spec.destination) : active.spec.source;
  assert.equal(expected.encoding, 'utf8');
  assert.equal(path.resolve(row.path), path.resolve(expected.value));
  if (receiver && phase !== 'completed') {
    contained(receive, row.path, false);
    contained(receive, path.dirname(row.path));
  } else contained(receiver ? receive : source, row.path);
  assert.equal(row.direction, receiver ? 'receive' : 'send');
  assert(Number.isSafeInteger(row.size) && Number.isSafeInteger(row.offset) && row.offset >= 0 && row.offset <= row.size);
  assert.equal(row.size, Number(active.spec.size));
  return { row, context, request };
}
try {
  const original = originalRow(); report.original = original;
  assertManifest(manifest(source), prepared.source_manifest, 'Independent original target source');
  if (phase === 'pause') {
    if (receiver) {
      assert.equal(original.row.state, 'paused'); assert(original.row.offset > 0 && original.row.offset < original.row.size);
      assert.equal(original.row.offset, Number(active.confirmed), 'Receiver flushed checkpoint differs from acknowledged UI bytes');
      assert(!fs.existsSync(original.row.path), 'Paused data was prematurely published as a final file');
    } else {
      assert(['paused', 'completed'].includes(original.row.state));
      assert(original.row.offset >= Number(active.confirmed) && original.row.offset <= original.row.size, 'Sender may be ahead, never behind the durable receiver checkpoint');
      if (original.row.state === 'completed') assert.equal(original.row.offset, original.row.size);
    }
    report.samples = [];
    const since = performance.now();
    for (let sample = 0; sample < 6; sample++) {
      if (sample) await new Promise(resolve => setTimeout(resolve, 1000));
      const current = originalRow();
      assert(current.row.state === original.row.state && current.row.offset === original.row.offset);
      report.samples.push({ elapsed_ms: performance.now() - since, state: current.row.state, offset: current.row.offset });
    }
    assert(report.samples.at(-1).elapsed_ms >= 5000);
  } else if (phase === 'completed') {
    assert.equal(original.row.state, 'completed');
    assert.equal(original.row.offset, original.row.size); assert.equal(original.request.resume_from, active.previous_attempt);
    assert(active.previous_attempt && active.previous_attempt !== active.attempt);
    const previous = db.prepare('SELECT request_id,state,size,offset FROM transfer_operations WHERE request_id=?').get(active.previous_attempt);
    assert(previous && previous.offset > 0 && (previous.state === 'paused' && (receiver ? previous.offset < previous.size : previous.offset <= previous.size) || !receiver && previous.state === 'completed' && previous.offset === previous.size));
    assert(fs.lstatSync(original.row.path).isFile() && !fs.lstatSync(original.row.path).isSymbolicLink());
    const digest = shaFile(original.row.path), expected = (receiver ? offer.expected_manifest : prepared.source_manifest).find(entry => entry.path.endsWith('random.bin')).sha256;
    assert.equal(digest, expected); assert.equal(original.row.sha256, expected);
    if (receiver) assertManifest(manifest(receive), offer.expected_manifest, 'Independent resumed-upload final directory');
    else assertManifest(control.local_received_manifest, prepared.source_manifest, 'Controller recorded resumed download matches independent target source');
    report.previous_attempt = previous; report.final_sha256 = digest;
  } else {
    assert.equal(original.row.state, 'cancelled');
    assert(original.row.offset > 0 && original.row.offset < original.row.size);
    if (receiver) {
      assert.equal(original.row.offset, Number(active.confirmed)); assert(!fs.existsSync(original.row.path), 'Cancelled upload final file must remain unpublished');
      report.partial_manifest = manifest(receive);
      const parent = path.relative(receive, path.dirname(original.row.path)).replaceAll('\\', '/');
      const parts = report.partial_manifest.filter(entry => entry.kind === 'file' && path.posix.dirname(entry.path.replaceAll('\\', '/')) === parent && entry.path.endsWith('.part'));
      assert.equal(parts.length, 1, 'Fresh cancelled receiver has one unambiguous partial'); assert.equal(parts[0].bytes, original.row.offset);
      report.partial = parts[0];
    }
    report.after_cancel_offset = original.row.offset;
    await new Promise(resolve => setTimeout(resolve, 5000));
    const current = originalRow(); assert(current.row.state === 'cancelled' && current.row.offset === original.row.offset);
    assertManifest(manifest(source), prepared.source_manifest, 'Target source after cancelled download');
    if (receiver) assertManifest(manifest(receive), report.partial_manifest, 'Cancelled receiver partial stopped changing');
  }
  report.pass = true;
} catch (error) { report.error = String(error.message || error); process.exitCode = 1; }
finally {
  db.close(); report.finished = new Date().toISOString();
  writeJson(output, report);
}
console.log(JSON.stringify({ pass: report.pass, phase, error: report.error || null, report: output }));
