import React, { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import FileManagerWindow from "../src/FileManagerWindow";
import type { FileOperation, NativePath } from "../src/fileManagerApi";
import { utf8Path } from "../src/fileManagerApi";
localStorage.setItem("pab.language", "en");
const callbacks = new Map<number, (value: unknown) => void>(), listeners = new Map<string, number[]>();
let next = 1;
const snapshots = new Map<string, NativePath>(), requests = new Map<string, any>();
const fixture = (window as any).fixture = {
  calls: [] as { command: string; args: any }[], batches: [] as any[], items: [] as any[], closed: [] as string[], holdNextScan: false,
  held: [] as { id: string; path: NativePath }[],
  nativeDrops: new Set<string>(), nativeDropLostReply: false,
  deletes: [] as string[], loseDeleteSuffix: "",
  nativeDirectory: null as NativePath | null,
  batchLostReply: false, holdBatchObservation: false, rejectNextBatch: false,
  holdTabClose: false,tabClosers:[] as (()=>void)[],
  holdBatchPoll: false, batchPollWaiters: [] as (() => void)[],
  holdBatchReply: false, rejectHeldBatch: false, batchReplyWaiters: [] as (() => void)[],
  entryNameOverrides:{} as Record<number,string>,
  snapshotNameOverrides: new Map<string, Record<number, string>>(),
  emit(event: string, payload: unknown) { if (event==="file-manager-open-tab" && payload && typeof payload==="object" && !("generation" in payload))payload={...payload,generation:crypto.randomUUID()};for (const id of listeners.get(event) || []) callbacks.get(id)?.({ event, payload }); },
  releaseScans() { for (const held of fixture.held.splice(0)) { snapshots.set(held.id, held.path); fixture.snapshotNameOverrides.set(held.id, { ...fixture.entryNameOverrides }); requests.set(held.id, frame(held.id, { result: "opened", snapshot_id: held.id })); } },
  ack(intent: string) { fixture.batches.forEach(batch => { batch.phase = intent === "pause" ? "paused" : "cancelled"; batch.revision = String(BigInt(batch.revision) + 1n); }); },
};
function frame(id: string, result: any) { return { request_id: id, state: "completed", reply: { request_id: id, result, execution_context: { path_style: "windows", identity: { account_name: "Native fixture user" } } }, message: null }; }
function makeEntry(index: number, parent: NativePath, names = fixture.entryNameOverrides) {
  const name = names[index] ?? (index === 0 ? "Folder" : `file-${String(index).padStart(6, "0")}.bin`);
  return { id: `entry-${index}`, name, path: index === 0 && fixture.nativeDirectory ? fixture.nativeDirectory : utf8Path(`${parent.value}\\${name}`), kind: index === 0 ? "directory" : "file", size: index === 0 ? null : "9007199254740993", modified_at_unix_ms: 1793000000000, hidden: false, readonly: false, unix_mode: null, error: null };
}
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "file-manager-fixture" }, currentWebview: { label: "file-manager-fixture" } },
  transformCallback(callback: (value: unknown) => void) { const id = next++; callbacks.set(id, callback); return id; }, unregisterCallback(id: number) { callbacks.delete(id); },
  invoke: async (command: string, args: any = {}) => {
    fixture.calls.push({ command, args });
    if (command === "plugin:event|listen") { const ids = listeners.get(args.event) || []; listeners.set(args.event, [...ids, args.handler]); return next++; }
    if (command.startsWith("plugin:")) return null;
    if (command === "file_manager_tabs") return [{ code: "428358431", name: "90", os_family: "windows",generation:"10000000-0000-0000-0000-000000000090" }, { code: "559038160", name: "Company", os_family: "windows",generation:"10000000-0000-0000-0000-000000000091" }];
    if (command === "file_manager_close_tab") {if (fixture.holdTabClose)await new Promise<void>(resolve=>fixture.tabClosers.push(resolve));return null;}
    if (command === "file_manager_local_identity") return "Native local fixture user";
    if (command === "file_manager_reveal_local") return null;
    if (command === "file_manager_link_status") return { connected: true, path: args.forceRelay ? "relay" : "p2p", rtt_ms: 12 };
    if (command === "file_manager_contexts" && new URLSearchParams(location.search).has("android")) {
      if (new URLSearchParams(location.search).has("unsupported")) throw new Error("This Android feature is not available");
      return { state: "completed", data: { type: "execution_contexts", backend: "android_app_uid", entries: [{mode:"service",account_name:"com.pixels.agent",account_id:"uid:10234",session_id:null,selection:{mode:"service"},unavailable_reason:null,identity:{mode:"service",account_name:"com.pixels.agent",account_id:"uid:10234",home:"/workspace",primary_group:10234,session_id:null,logon_id:null,environment_source:"service_process"}}] } };
    }
    if (command === "file_manager_contexts") return { state: "completed", data: { type: "execution_contexts", entries: args.forceRelay && (fixture as any).hideRelayUser ? [] : [{ mode: "user", account_name: "Test user", account_id: "uid:1001", session_id: "1", identity: { mode: "user", account_id: "uid:1001", account_name: "Test user", home: "C:\\Users\\fixture", primary_group:null, session_id:"1",logon_id:(fixture as any).nativeLogon || "fixture-login",environment_source:"native_account" }, selection: { mode: "user", context_ref: `user-${args.forceRelay ? "relay" : "auto"}${(fixture as any).nativeGeneration ? `-${(fixture as any).nativeGeneration}` : ""}` },unavailable_reason:null }] } };
    if (command === "file_manager_clear_completed") {
      if ((fixture as any).failClearCompleted) throw new Error("Could not clear completed transfers");
      const ids = fixture.batches.filter(batch => String(batch.spec.code) === args.code && ["completed", "completed_with_skips"].includes(batch.phase)).map(batch => batch.spec.id);
      fixture.batches = fixture.batches.filter(batch => !ids.includes(batch.spec.id)); return ids;
    }
    if (command === "file_manager_batches") {
      if (fixture.holdBatchPoll) return new Promise(resolve => fixture.batchPollWaiters.push(() => resolve(structuredClone(fixture.batches))));
      return fixture.holdBatchObservation ? [] : structuredClone(fixture.batches);
    }
    if (command === "file_manager_batch_items") return structuredClone(fixture.items.filter(item => !args.filter || item.state === args.filter).slice(args.after, args.after + 128));
    if (command === "file_manager_resolve_conflict") {
      if ((fixture as any).failResolveConflict) throw new Error("Could not resolve conflict");
      const item = fixture.items.find(item => item.spec.id === args.item);
      if (!item || item.state !== "waiting_conflict") throw new Error("Not a waiting conflict");
      for (const target of args.applyRemaining ? fixture.items.filter(item => item.state === "waiting_conflict") : [item]) {
        target.state = args.choice === "skip" ? "skipped" : "queued";
        if (args.choice === "keep_both") target.effective_destination = utf8Path("D:\\destination\\中文 (1).txt");
      }
      const batch = fixture.batches.find(batch => batch.spec.id === args.batch);
      batch.phase = fixture.items.some(item => item.state === "waiting_conflict") ? "waiting_conflict" : "running";
      return structuredClone(batch);
    }
    if (command === "file_manager_start_batch") {
      if (fixture.rejectNextBatch) { fixture.rejectNextBatch = false; throw { message: "Destination denied", admitted: false }; }
      let batch = fixture.batches.find(item => item.spec.id === args.id);
      if (!batch) { batch = { spec: { id: args.id, code: args.code, direction: args.direction, destination: args.destination, execution: args.execution, force_relay: args.forceRelay }, phase: "running", intent: "run", revision: "1", total: "1", completed: "0", skipped: "0", failed: "0", cancelled: "0", size: "1000", confirmed: "500", scan_complete: true, message: null }; fixture.batches.push(batch); }
      if (fixture.batchLostReply) { fixture.batchLostReply = false; throw new Error("Acceptance reply lost"); }
      if (fixture.holdBatchReply) {
        await new Promise<void>(resolve => fixture.batchReplyWaiters.push(resolve));
        if (fixture.rejectHeldBatch) throw new Error("Old account submission failed");
      }
      return structuredClone(batch);
    }
    if (command === "file_manager_start_drop") {
      if (!fixture.nativeDrops.has(args.dropId)) throw new Error("Native drop token is not owned by this window");
      let batch = fixture.batches.find(item => item.spec.id === args.id);
      if (!batch) { batch = { spec: { id: args.id, code: args.code, direction: "upload" }, phase: "running", intent: "run", revision: "1", total: "2", completed: "0", skipped: "0", failed: "0", cancelled: "0", size: "10", confirmed: "0", scan_complete: true, message: null }; fixture.batches.push(batch); }
      if (fixture.nativeDropLostReply) { fixture.nativeDropLostReply = false; throw new Error("Acceptance reply lost"); }
      return structuredClone(batch);
    }
    if (command === "file_manager_batch_intent") { const batch = fixture.batches.find(item => item.spec.id === args.id); batch.intent = args.intent; batch.phase = args.intent === "pause" ? "pause_requested" : args.intent === "cancel" ? "cancel_requested" : "running"; batch.revision = String(BigInt(batch.revision) + 1n); return structuredClone(batch); }
    if (command === "file_manager_retry_failed") { const batch = fixture.batches.find(item => item.spec.id === args.id); batch.failed = "0"; batch.phase = "running"; batch.revision = String(BigInt(batch.revision) + 1n); return structuredClone(batch); }
    if (command === "file_manager_status") return requests.get(args.requestId);
    if (command === "file_manager_cancel") { const old = requests.get(args.requestId); old.state = "cancel_requested"; return old; }
    if (command === "file_manager_path") { if (args.path.encoding !== "utf8") return null; if (/^(?:\\\\\?\\)?[a-z]:\\$/i.test(args.path.value)) return null; const value = args.path.value.replace(/\\[^\\]+\\?$/, ""); return utf8Path(/[a-z]:$/i.test(value) ? value + "\\" : value.length >= 3 ? value : "C:\\"); }
    if (command === "file_manager_request") {
      const op: FileOperation = args.operation, id: string = args.requestId;
      if (op.action === "create_directory") {
        if (op.name.value.includes("/")) throw { message: "expected a single native filename", admitted: false };
        if ((fixture as any).rejectMutationBusy) { (fixture as any).rejectMutationBusy = false; throw { message: "File window is busy", admitted: false }; }
      }
      const currentNativeReference = `user-${args.forceRelay ? "relay" : "auto"}${(fixture as any).nativeGeneration ? `-${(fixture as any).nativeGeneration}` : ""}`;
      if (args.endpoint !== "local" && args.execution?.mode === "user" && op.action !== "close_directory" && ((fixture as any).rejectNativeReads || args.execution.context_ref !== currentNativeReference)) throw new Error('Executor returned an unexpected task response: Error { code: EnvironmentChanged, message: "unknown execution context; query available contexts again" }');
      let result: any;
      if (op.action === "locations") result = { result: "locations", locations: { path_style: "windows", home: utf8Path("C:\\Fixtures"), roots: [{ name: "C:\\", path: utf8Path("C:\\") }, { name: "D:\\", path: utf8Path("D:\\") }] } };
      else if (op.action === "open_directory") {
        if (fixture.holdNextScan) { fixture.holdNextScan = false; fixture.held.push({ id, path: op.path }); const pending = { request_id: id, state: "running", reply: null, message: null }; requests.set(id, pending); return pending; }
        snapshots.set(id, op.path); fixture.snapshotNameOverrides.set(id, { ...fixture.entryNameOverrides }); result = { result: "opened", snapshot_id: id };
      } else if (op.action === "close_directory") { fixture.closed.push(op.snapshot_id); result = { result: "closed", snapshot_id: op.snapshot_id }; }
      else if (op.action === "read_directory") {
        const parent = snapshots.get(op.snapshot_id)!;
        const names = fixture.snapshotNameOverrides.get(op.snapshot_id) || {};
        const total = parent.value.endsWith("Folder") ? 0 : 100_000;
        let indices = Array.from({ length: total }, (_, i) => i);
        if (op.filter.query) indices = indices.filter(index => makeEntry(index, parent, names).name.includes(op.filter.query));
        if (op.filter.descending) indices.reverse();
        const start = Number(op.after || 0), slice = indices.slice(start, start + op.limit);
        result = { result: "directory", page: { snapshot_id: op.snapshot_id, path: parent, entries: slice.map(index => makeEntry(index, parent, names)), next_after: start + slice.length < indices.length ? String(start + slice.length) : null, total_entries: String(indices.length), complete: true, observed_at_unix_ms: 1793000000000 } };
      } else if (op.action === "inspect") result = { result: "entry", entry: { ...makeEntry(1, utf8Path("C:\\Fixtures")), path: op.path } };
      else result = { result: "applied", path: "path" in op ? op.path : op.parent };
      const response = frame(id, result); requests.set(id, response);
      if (op.action === "delete") {
        fixture.deletes.push(op.path.value);
        if (fixture.loseDeleteSuffix && op.path.value.endsWith(fixture.loseDeleteSuffix)) { fixture.loseDeleteSuffix = ""; throw new Error("Mutation reply lost after completion"); }
      }
      return response;
    }
    throw new Error(`Unhandled fixture command: ${command}`);
  },
};
createRoot(document.getElementById("root")!).render(<StrictMode><FileManagerWindow /></StrictMode>);
