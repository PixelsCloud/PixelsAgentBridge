import brand from "./assets/brand.svg";
import { useEffect, useRef, useState, type DragEvent, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowDown, ArrowUp, ArrowLeft, ArrowRight, ChevronDown, ChevronUp, Maximize2, Minus, Pause, Play, X } from "lucide-react";
import { FilePane, type PaneSelection } from "./FilePane";
import { FileBatchDialog } from "./FileBatchDialog";
import type { DirectoryTransfer } from "./fileDirectoryRefresh";
import { fileSize, pathLabel, type NativePath } from "./fileManagerApi";
import { TransferRateTracker, unknownTransferRate, remainingTime, type TransferRate } from "./fileTransferRate";
import { initialLanguage } from "./i18n";
import type { ExecutionEntry, ExecutionReply, ExecutionSelection } from "./executionQueries";
import "./theme.css";
import "./fileManager.css";

type Tab = { code: string; name: string; os_family: string | null; generation: string };
type Dropped = { drop_id: string; id: string; count: number; names: string[]; code: string; destination: NativePath; execution: ExecutionSelection; conflict: string; forceRelay: boolean };
type DropNotice = { drop_id: string; count: number; names: string[]; x: number; y: number };
type Submission = { id: string; code: string; direction: "upload" | "download"; sources: NativePath[]; destination: NativePath; execution: ExecutionSelection; conflict: string; forceRelay: boolean };
type Batch = DirectoryTransfer & { intent: "run" | "pause" | "cancel"; revision: string; total: string; skipped: string; failed: string; cancelled: string; size: string; confirmed: string; active_elapsed_ms?: string; scan_complete: boolean; message: string | null; runner_active?: boolean };
const en = initialLanguage() === "en";
const words: Record<string, string> = en ? { running: "Transferring", scanning: "Scanning", queued: "Queued", paused: "Paused", pausing: "Pausing…", pause_requested: "Pausing…", cancel_requested: "Cancelling…", completed_with_skips: "Completed with skips", partial_failed: "Some items failed", cancelling: "Cancelling…", cancelled: "Cancelled", completed: "Completed", failed: "Failed", unconfirmed: "Unconfirmed", reconciling: "Reconciling", waiting_conflict: "Name conflict", interrupted: "Interrupted" } : { running: "传输中", scanning: "正在扫描", queued: "排队中", paused: "已暂停", pausing: "暂停中…", pause_requested: "暂停中…", cancel_requested: "取消中…", completed_with_skips: "已完成，部分跳过", partial_failed: "部分项目失败", cancelling: "取消中…", cancelled: "已取消", completed: "已完成", failed: "失败", unconfirmed: "待确认", reconciling: "正在核实", waiting_conflict: "名称冲突", interrupted: "传输中断" };
Object.assign(words, en ? { pause_requested: "Pausing…", cancel_requested: "Cancelling…", completed_with_skips: "Completed with skips", partial_failed: "Partially failed", verifying: "Verifying", publishing: "Publishing" } : { pause_requested: "暂停中…", cancel_requested: "取消中…", completed_with_skips: "完成（有跳过）", partial_failed: "部分失败", verifying: "校验中", publishing: "发布中" });
const terminal = (batch: Batch) => ["completed", "completed_with_skips", "partial_failed", "failed", "cancelled"].includes(batch.phase);

function nativeUserKey(entry:ExecutionEntry, android = false):string|null {
  const identity=entry.identity as (NonNullable<ExecutionEntry["identity"]> & {primary_group?:number|null;environment_source?:string})|null;
  const allowed = identity?.mode === "user" || android && entry.selection?.mode === "service" && identity?.mode === "service" && /^uid:[1-9][0-9]*$/.test(identity.account_id) && identity.home === "/workspace";
  return allowed && identity && identity.account_id && identity.home ? JSON.stringify([identity.mode,identity.account_id,identity.account_name,identity.home,identity.primary_group??null,identity.session_id,identity.logon_id,identity.environment_source??"native_account"]):null;
}
function RemotePane({ tab, active, ended, forceRelay, batchUpdates, onSelection, onExecution, onDragSelection, transferAction, relayControl }: { relayControl: ReactNode; transferAction: ReactNode; tab: Tab; active: boolean; ended: boolean; forceRelay: boolean; batchUpdates: readonly DirectoryTransfer[]; onSelection: (selection: PaneSelection) => void; onExecution: (code: string, value: ExecutionSelection|null) => void; onDragSelection: (selection: PaneSelection | null, token: string | null) => void }) {
  const [contexts, setContexts] = useState<{ relay:boolean;android?:boolean;entries: ExecutionEntry[]; loading: boolean; error: string }>({ relay:forceRelay,entries: [], loading: false, error: "" });
  const [selectedUser, setSelectedUser] = useState<{key:string;label:string}|null>(null);
  const [contextRevision, setContextRevision] = useState(0);
  const refreshedReferences = useRef(new Set<string>()), lastDiscovery = useRef(0);
  const [link, setLink] = useState<{ connected: boolean; path: string; rtt_ms: number | null } | null>(null);
  useEffect(() => {
    if (!active || ended) return;
    let alive = true, timer: ReturnType<typeof setTimeout>;
    setLink(null);
    const poll = async () => {
      try { const value = await invoke<{ connected: boolean; path: string; rtt_ms: number | null }>("file_manager_link_status", { code: tab.code, forceRelay }); if (alive) setLink(value); }
      catch { if (alive) setLink(null); }
      finally { if (alive) timer = setTimeout(() => void poll(), 2000); }
    };
    void poll(); return () => { alive = false; clearTimeout(timer); };
  }, [active, tab.code, forceRelay, ended]);
  useEffect(() => { refreshedReferences.current.clear(); lastDiscovery.current = 0; }, [tab.code, forceRelay, ended]);
  useEffect(() => {
    let alive = true;
    const recovering = contextRevision > 0 && contexts.relay === forceRelay && contexts.entries.length > 0;
    if (!recovering) setContexts({ relay:forceRelay,entries: [], loading: true, error: "" });
    // A failed reference is rediscovered once, without clearing the pane or
    // repeatedly mounting skeletons. Connection churn is bounded and cooled.
    const discover = () => {
      if (ended) return;
      lastDiscovery.current = performance.now();
      void invoke<ExecutionReply>("file_manager_contexts", { code: tab.code, forceRelay }).then(reply => {
        if (!alive) return;
        if (reply.state !== "completed" || reply.data?.type !== "execution_contexts") throw new Error(reply.error || reply.state);
        const android = reply.data.backend === "android_app_uid";
        const users = reply.data.entries.filter(entry => !entry.unavailable_reason && nativeUserKey(entry, android));
        setSelectedUser(current => current || (users.length === 1 ? { key: nativeUserKey(users[0], android)!, label: users[0].account_name } : null));
        setContexts({ relay:forceRelay,android,entries: users, loading: false, error: users.length === 1 ? "" : (en ? "Console user is unavailable" : "Console 用户不可用") });
      }).catch(cause => { if (alive) setContexts(current => ({ relay:forceRelay,entries: recovering ? current.entries : [], loading: false, error: String(cause) })); });
    };
    const timer = recovering ? setTimeout(discover, Math.max(0, 2000 - (performance.now() - lastDiscovery.current))) : undefined;
    if (!recovering) discover();
    return () => { alive = false; if (timer !== undefined) clearTimeout(timer); };
  }, [tab.code, forceRelay, ended, contextRevision]);
  const entries=contexts.relay===forceRelay && !contexts.loading ? contexts.entries:[];
  const selectedEntry=selectedUser ? entries.find(entry=>nativeUserKey(entry, contexts.android)===selectedUser.key):null;
  const execution=selectedEntry?.selection;
  useEffect(() => {
    onSelection({ endpoint: tab.code, execution: execution || { mode: "service" }, path: null, entries: [], pathStyle: tab.os_family === "windows" ? "windows" : "posix" });
    onDragSelection(null, null);
    onExecution(tab.code, execution || null);
  }, [tab.code, forceRelay, JSON.stringify(execution)]);
  return <div className="fm-remote-tab" hidden={!active} data-execution-ref={execution && "context_ref" in execution ? execution.context_ref : "__unavailable"}>
    {(contexts.error || !contexts.loading && selectedUser && !selectedEntry) && <div className="fm-error" role="alert">{contexts.error || (en ? "Console session changed. Reopen this device tab." : "Console 登录会话已变化，请重新打开此设备页签。")}</div>}
    <FilePane key={`${forceRelay}:${selectedUser?.key ?? "service"}`} endpoint={tab.code} execution={execution || { mode: "service" }} executionIdentityKey={selectedUser?.key ?? "service"} onExecutionUnavailable={() => {
      if (ended || execution?.mode !== "user" || refreshedReferences.current.has(execution.context_ref) || refreshedReferences.current.size >= 32) return;
      refreshedReferences.current.add(execution.context_ref); setContextRevision(value => value + 1);
    }} forceRelay={forceRelay} title={tab.name} headingDetail={<><span className="fm-link-status">({link?.connected ? `${link.path === "p2p" ? "P2P" : link.path === "relay" ? "Relay" : en ? "Connected" : "已连接"}${link.rtt_ms === null ? "" : ` · ${link.rtt_ms} ms`}` : en ? "Awaiting connection" : "等待连接"})</span>{relayControl}</>} transferAction={transferAction} disabled={ended || !execution} batchUpdates={batchUpdates} onSelection={onSelection} onDragSelection={onDragSelection} />
  </div>;
}

export default function FileManagerWindow() {
  const appWindow = useRef(getCurrentWindow()).current;
  const [tabState,setTabState]=useState<{tabs:Tab[];active:string}>({tabs:[],active:""});
  const {tabs,active}=tabState;
  const setActive=(active:string)=>setTabState(current=>({...current,active}));
  const activeGeneration=tabs.find(tab=>tab.code===active)?.generation || "";
  const [local, setLocal] = useState<PaneSelection | null>(null), [remotes, setRemotes] = useState<Record<string, PaneSelection>>({});
  const [executions, setExecutions] = useState<Record<string, ExecutionSelection>>({});
  const [batches, setBatches] = useState<Batch[]>([]), [error, setError] = useState("");
  const [ended, setEnded] = useState(false), [starting, setStarting] = useState(false), [forceRelay, setForceRelay] = useState(false);
  const [rateTracker] = useState(() => new TransferRateTracker());
  const [rates, setRates] = useState<Record<string, TransferRate>>({});
  const [clearingCompleted, setClearingCompleted] = useState(false);
  const clearCompletedGate = useRef(false);
  const conflict = "ask";
  const [closeDialog, setCloseDialog] = useState(false);
  const [details, setDetails] = useState<Batch | null>(null);
  const [dropped, setDropped] = useState<Dropped | null>(null);
  const [pendingSubmission, setPendingSubmission] = useState<Submission | null>(null);
  const [closingTab,setClosingTab]=useState<string|null>(null);
  const tabCloseGate=useRef(false);
  const paneDrag=useRef<{selection:PaneSelection;token:string;forceRelay:boolean}|null>(null);
  const [dropSide,setDropSide]=useState<"local"|"remote"|null>(null);
  const remoteBox = useRef<HTMLDivElement>(null), dropForm = useRef<HTMLDivElement>(null), startGate = useRef(false);
  const [taskHeight, setTaskHeight] = useState(190), [tasksCollapsed, setTasksCollapsed] = useState(false), [taskFilter, setTaskFilter] = useState("all");
  const resizing = useRef<{ y: number; height: number } | null>(null);
  const maxTaskHeight = Math.max(95, Math.floor(window.innerHeight / 2));
  const resizeTasks = (height: number) => setTaskHeight(Math.max(95, Math.min(maxTaskHeight, height)));
  const [dark, setDark] = useState(() => localStorage.getItem("pab.theme") === "dark");
  const closed = useRef(false), pollFailure = useRef(false), snapshotEpoch = useRef(0);
  // End the window synchronously: old IPC promises may settle before React rerenders.
  // This window never adopts the next account; it must be opened with a new owner.
  const sessionEnded = useRef(false);
  useEffect(() => {
    if (ended) { rateTracker.clear(); setRates({}); return; }
    const now = performance.now();
    rateTracker.retain(batches.map(batch => batch.spec.id));
    setRates(Object.fromEntries(batches.map(batch => [batch.spec.id, rateTracker.observe(batch, now)])));
  }, [batches, ended, rateTracker]);
  useEffect(() => {
    if (ended) return;
    const timer = setInterval(() => {
      const now = performance.now();
      setRates(current => {
        let next = current;
        for (const [id, rate] of Object.entries(current)) {
          if (rate.bytesPerSecond !== null && !rateTracker.fresh(id, now)) {
            if (next === current) next = { ...current };
            next[id] = unknownTransferRate;
          }
        }
        return next;
      });
    }, 250);
    const visibility = () => { rateTracker.clear(); setRates({}); };
    document.addEventListener("visibilitychange", visibility);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", visibility); };
  }, [ended, rateTracker]);
  const remote = remotes[activeGeneration];
  const dropTarget = useRef({ active, remote, execution: executions[activeGeneration], conflict, forceRelay, ended, submissionPending:!!pendingSubmission });
  dropTarget.current = { active, remote, execution: executions[activeGeneration], conflict, forceRelay, ended, submissionPending:!!pendingSubmission };
  const ongoing = batches.some(batch => !terminal(batch));
  useEffect(() => { if (pendingSubmission && batches.some(batch => batch.spec.id === pendingSubmission.id)) { setPendingSubmission(null); setError(""); } }, [pendingSubmission, batches]);
  useEffect(() => { document.documentElement.dataset.theme = dark ? "dark" : "light"; }, [dark]);
  useEffect(() => {
    let alive = true;
    const listener = listen<DropNotice>("file-manager-native-drop", event => {
      const target = dropTarget.current, bounds = remoteBox.current?.getBoundingClientRect(), notice = event.payload;
      const x = notice.x / window.devicePixelRatio, y = notice.y / window.devicePixelRatio;
      if (!alive || sessionEnded.current || target.ended || target.submissionPending || startGate.current || !bounds || !target.remote?.path || !target.execution || x < bounds.left || x > bounds.right || y < bounds.top || y > bounds.bottom) return;
      setDropped({ drop_id: notice.drop_id, id: crypto.randomUUID(), count: notice.count, names: notice.names, code: target.active, destination: target.remote.path, execution: target.execution, conflict: target.conflict, forceRelay: target.forceRelay });
    });
    return () => { alive = false; void listener.then(stop => stop()); };
  }, []);
  useEffect(() => {
    if (!dropped) return;
    const previous = document.activeElement as HTMLElement | null;
    dropForm.current?.querySelector<HTMLElement>("button")?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !startGate.current) setDropped(null);
      if (event.key !== "Tab") return;
      const targets = [...(dropForm.current?.querySelectorAll<HTMLElement>("button:enabled") || [])], index = targets.indexOf(document.activeElement as HTMLElement);
      if (event.shiftKey && index <= 0) { event.preventDefault(); targets[targets.length - 1]?.focus(); }
      else if (!event.shiftKey && (index < 0 || index === targets.length - 1)) { event.preventDefault(); targets[0]?.focus(); }
    };
    document.addEventListener("keydown", key); return () => { document.removeEventListener("keydown", key); if (previous?.isConnected) previous.focus(); };
  }, [dropped]);
  useEffect(() => {
    let alive = true;
    const listeners = [listen<Tab>("file-manager-open-tab", event => { if (!alive || sessionEnded.current) return; setTabState(current=>({tabs:[...current.tabs.filter(tab=>tab.code!==event.payload.code),event.payload],active:event.payload.code})); }), listen("file-manager-account-ended", () => {
      if (!alive || sessionEnded.current) return;
      sessionEnded.current = true;
      snapshotEpoch.current++;
      paneDrag.current = null; resizing.current = null;
      rateTracker.clear(); setRates({});
      setTabState({ tabs: [], active: "" }); setBatches([]);
      setLocal(null); setRemotes({}); setExecutions({});
      setDetails(null); setDropped(null); setPendingSubmission(null);
      setCloseDialog(false); setDropSide(null); setClosingTab(null); setStarting(false);
      setEnded(true);
      setError(en ? "Account changed. Reopen file transfer under the current account." : "账号已切换，请在当前账号下重新打开文件传输。");
    })];
    void invoke<Tab[]>("file_manager_tabs").then(value => { if (alive && !sessionEnded.current) { setTabState(current=>({tabs:[...value,...current.tabs.filter(tab=>!value.some(item=>item.code===tab.code))],active:current.active || value[0]?.code || ""})); } }).catch(cause => { if (alive && !sessionEnded.current) setError(String(cause)); });
    const storage = (event: StorageEvent) => { if (event.key === "pab.theme") setDark(event.newValue === "dark"); };
    window.addEventListener("storage", storage);
    return () => { alive = false; for (const listener of listeners) void listener.then(stop => stop()); window.removeEventListener("storage", storage); };
  }, []);
  useEffect(() => {
    if (ended) return;
    let alive = true, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const epoch = snapshotEpoch.current;
        const values = await invoke<Batch[]>("file_manager_batches");
        if (alive && !sessionEnded.current && epoch === snapshotEpoch.current) setBatches(current => values.map(next => { const previous = current.find(item => item.spec.id === next.spec.id); return previous && BigInt(previous.revision) > BigInt(next.revision) ? previous : next; }));
        pollFailure.current = false;
      } catch (cause) { if (alive && !sessionEnded.current && !pollFailure.current) { pollFailure.current = true; setError(String(cause)); } }
      finally { if (alive && !sessionEnded.current) timer = setTimeout(() => void poll(), 750); }
    };
    void poll();
    return () => { alive = false; clearTimeout(timer); };
  }, [ended]);
  async function start(direction: "upload" | "download") {
    const source = direction === "upload" ? local : remote, destination = direction === "upload" ? remote : local;
    if (startGate.current || pendingSubmission || ended || !source?.entries.length || !destination?.path || !executions[activeGeneration]) return;
    await submit({ id: crypto.randomUUID(), code: active, direction, sources: source.entries.map(entry => entry.path), destination: destination.path, execution: executions[activeGeneration], conflict, forceRelay });
  }
  function beginPaneDrag(selection:PaneSelection|null,token:string|null) {if(sessionEnded.current)return;paneDrag.current=selection && token ? {selection,token,forceRelay}:null;if (!selection)setDropSide(null);}
  function dragAllowed(side:"local"|"remote",event:DragEvent) {
    const drag=paneDrag.current;
    return !!drag && drag.forceRelay === forceRelay && !sessionEnded.current && !ended && !pendingSubmission && !starting && !dropped && event.dataTransfer.types.includes("application/x-pixels-file-selection") && (side==="local" ? drag.selection.endpoint!=="local" && !!local?.path : drag.selection.endpoint==="local" && !!remote?.path && !!executions[activeGeneration]);
  }
  function paneDragOver(side:"local"|"remote",event:DragEvent) {if (dragAllowed(side,event)) {event.preventDefault();event.dataTransfer.dropEffect="copy";setDropSide(side);}}
  async function paneDrop(side:"local"|"remote",event:DragEvent) {
    if (!dragAllowed(side,event))return;
    const drag=paneDrag.current!;
    if (event.dataTransfer.getData("application/x-pixels-file-selection")!==drag.token)return;
    event.preventDefault();paneDrag.current=null;setDropSide(null);
    const upload=side==="remote",destination=upload?remote!.path!:local!.path!;
    await submit({id:crypto.randomUUID(),code:upload?active:drag.selection.endpoint,direction:upload?"upload":"download",sources:drag.selection.entries.map(entry=>entry.path),destination,execution:upload?executions[activeGeneration]:drag.selection.execution,conflict,forceRelay:upload?forceRelay:drag.forceRelay});
  }
  async function submit(submission: Submission) {
    if (startGate.current || sessionEnded.current || ended) return;
    startGate.current = true;
    setPendingSubmission(submission);
    setStarting(true); setError("");
    try {
      const batch = await invoke<Batch>("file_manager_start_batch", submission);
      if (sessionEnded.current) return;
      snapshotEpoch.current++;
      setBatches(current => [batch, ...current.filter(item => item.spec.id !== batch.spec.id)]);
      setPendingSubmission(null);
    } catch (cause) {
      if (sessionEnded.current) return;
      if (cause && typeof cause === "object" && "admitted" in cause && cause.admitted === false) setPendingSubmission(null);
      setError(`${cause && typeof cause === "object" && "message" in cause ? String(cause.message) : String(cause)} · ${submission.id}`);
    }
    finally { startGate.current = false; setStarting(false); }
  }
  async function startDrop() {
    if (!dropped || startGate.current || pendingSubmission || sessionEnded.current || ended) return;
    startGate.current = true; setStarting(true); setError("");
    try { const batch = await invoke<Batch>("file_manager_start_drop", { id: dropped.id, dropId: dropped.drop_id, code: dropped.code, destination: dropped.destination, execution: dropped.execution, conflict: dropped.conflict, forceRelay: dropped.forceRelay }); if (sessionEnded.current) return; snapshotEpoch.current++; setBatches(current => [batch, ...current.filter(item => item.spec.id !== batch.spec.id)]); setDropped(null); }
    catch (cause) { if (!sessionEnded.current) setError(`${String(cause)} · ${dropped.id}`); }
    finally { startGate.current = false; setStarting(false); }
  }
  async function control(id: string, intent: "run" | "pause" | "cancel") {
    if (sessionEnded.current) return;
    try { const batch = await invoke<Batch>("file_manager_batch_intent", { id, intent }); if (sessionEnded.current) return; snapshotEpoch.current++; setBatches(current => current.map(value => value.spec.id === id ? batch : value)); }
    catch (cause) { if (!sessionEnded.current) setError(String(cause)); }
  }
  async function retry(id: string) {
    if (sessionEnded.current) return;
    try { const batch = await invoke<Batch>("file_manager_retry_failed", { id }); if (sessionEnded.current) return; snapshotEpoch.current++; setBatches(current => current.map(value => value.spec.id === id ? batch : value)); }
    catch (cause) { if (!sessionEnded.current) setError(String(cause)); }
  }
  async function clearCompleted() {
    if (clearCompletedGate.current || sessionEnded.current) return;
    clearCompletedGate.current = true; setClearingCompleted(true); setError("");
    try {
      const ids = new Set(await invoke<string[]>("file_manager_clear_completed"));
      if (sessionEnded.current) return;
      snapshotEpoch.current++;
      setBatches(current => current.filter(batch => !ids.has(batch.spec.id)));
      setDetails(current => current && ids.has(current.spec.id) ? null : current);
    } catch (cause) { if (!sessionEnded.current) setError(String(cause)); }
    finally { clearCompletedGate.current = false; setClearingCompleted(false); }
  }
  const promptedConflicts = useRef(new Set<string>());
  useEffect(() => {
    const waiting = batches.filter(batch => batch.phase === "waiting_conflict");
    const ids = new Set(waiting.map(batch => batch.spec.id));
    for (const id of promptedConflicts.current) if (!ids.has(id)) promptedConflicts.current.delete(id);
    if (ended || details || dropped || closeDialog) return;
    const next = waiting.find(batch => !promptedConflicts.current.has(batch.spec.id));
    if (next) { promptedConflicts.current.add(next.spec.id); setDetails(next); }
  }, [batches, ended, details, dropped, closeDialog]);
  async function closeTab(tab:Tab) {
    const {code,generation}=tab;
    if (tabCloseGate.current || sessionEnded.current || ended)return;
    tabCloseGate.current=true;setClosingTab(code);
    try {
      await invoke("file_manager_close_tab",{code,generation});
      if (sessionEnded.current) return;
      setTabState(current=>{const index=current.tabs.findIndex(tab=>tab.generation===generation),remaining=current.tabs.filter(tab=>tab.generation!==generation);return {tabs:remaining,active:current.active===code && !remaining.some(tab=>tab.code===code)?remaining[Math.min(index,remaining.length-1)]?.code || "":current.active};});
      setRemotes(current=>{const next={...current};delete next[generation];return next;});setExecutions(current=>{const next={...current};delete next[generation];return next;});
    }catch(cause){if(!sessionEnded.current)setError(String(cause));}finally{tabCloseGate.current=false;setClosingTab(null);}
  }
  async function close() { if (closed.current) return; if (ongoing) { setCloseDialog(true); return; } closed.current = true; await appWindow.close(); }
  return <main className="fm-window">
    <header className="fm-titlebar" data-tauri-drag-region><div className="fm-titlebar-brand" data-tauri-drag-region><img src={brand} alt="Pixels" draggable={false} data-tauri-drag-region/></div><div className="fm-device-tabs" role="tablist" aria-label={en ? "Devices" : "设备"} onKeyDown={event=>{if (!["ArrowLeft","ArrowRight","Home","End"].includes(event.key))return;const items=[...event.currentTarget.querySelectorAll<HTMLButtonElement>("[role='tab']")],index=items.indexOf(document.activeElement as HTMLButtonElement);if (index<0)return;event.preventDefault();items[event.key==="Home"?0:event.key==="End"?items.length-1:(index+(event.key==="ArrowRight"?1:items.length-1))%items.length]?.focus();}}>{tabs.map(tab => <div key={tab.code} className="fm-device-tab" data-selected={active===tab.code}><button role="tab" aria-selected={active === tab.code} onClick={() => setActive(tab.code)}><span>{tab.name}</span><small>{tab.code}</small></button><button title={en ? `Close ${tab.name} tab` : `关闭 ${tab.name} 页签`} aria-label={en ? `Close ${tab.name} tab` : `关闭 ${tab.name} 页签`} disabled={ended || closingTab!==null} onClick={()=>void closeTab(tab)}><X size={13}/></button></div>)}</div><div className="fm-titlebar-space" data-tauri-drag-region/><nav><button title={en ? "Minimize" : "最小化"} onClick={() => void appWindow.minimize()}><Minus size={16} /></button><button title={en ? "Maximize" : "最大化"} onClick={() => void appWindow.toggleMaximize()}><Maximize2 size={15} /></button><button title={en ? "Close" : "关闭"} onClick={() => void close()}><X size={18} /></button></nav></header>
    {error && <div role="alert" className="fm-window-error">{error}<button title={en ? "Dismiss" : "关闭提示"} onClick={() => setError("")}><X size={15} /></button></div>}
    {pendingSubmission && <div className="fm-submission" role="status"><span>{en ? "Confirming original submission" : "正在核实原提交"} · {pendingSubmission.code} · {pendingSubmission.id}</span><button disabled={starting || ended} onClick={() => void submit(pendingSubmission)}>{en ? "Retry original submission" : "核实原提交"}</button></div>}
    {!ended && <div className="fm-panes"><div className={`fm-local-tab ${dropSide === "local" ? "fm-drop-active" : ""}`} onDragOver={event => paneDragOver("local",event)} onDrop={event => void paneDrop("local",event)} onDragLeave={event => {if (!event.currentTarget.contains(event.relatedTarget as Node | null))setDropSide(null);}}><FilePane endpoint="local" batchUpdates={batches} transferAction={<button className="primary" disabled={ended || starting || !!pendingSubmission || !local?.entries.length || !remote?.path || !executions[activeGeneration]} onClick={() => void start("upload")}><ArrowRight size={16} />{en ? "Send" : "发送"}{local?.entries.length ? ` (${local.entries.length})` : ""}</button>} title={en ? "This computer" : "本机"} disabled={ended} onSelection={selection => { if (!sessionEnded.current) setLocal(selection); }} onDragSelection={beginPaneDrag} /></div><div className={`fm-remote-panes ${dropSide === "remote" ? "fm-drop-active" : ""}`} ref={remoteBox} onDragOver={event => paneDragOver("remote",event)} onDrop={event => void paneDrop("remote",event)} onDragLeave={event => {if (!event.currentTarget.contains(event.relatedTarget as Node | null))setDropSide(null);}}>{tabs.map(tab => <RemotePane relayControl={<label className="fm-relay-toggle" hidden><input type="checkbox" disabled={ended} checked={forceRelay} onChange={event => { setForceRelay(event.target.checked); setRemotes({}); setExecutions({}); }} />{en ? "Force Relay" : "强制 Relay"}</label>} transferAction={<button className="primary" disabled={ended || starting || !!pendingSubmission || !remote?.entries.length || !local?.path || !executions[activeGeneration]} onClick={() => void start("download")}><ArrowLeft size={16} />{en ? "Receive" : "接收"}{remote?.entries.length ? ` (${remote.entries.length})` : ""}</button>} key={tab.generation} tab={tab} batchUpdates={batches} active={active === tab.code} ended={ended} forceRelay={forceRelay} onSelection={selection => { if (!sessionEnded.current) setRemotes(current => ({ ...current, [tab.generation]: selection })); }} onExecution={(_code, execution) => { if (!sessionEnded.current) setExecutions(current => {if(execution)return {...current,[tab.generation]:execution};if(!current[tab.generation])return current;const next={...current};delete next[tab.generation];return next;}); }} onDragSelection={beginPaneDrag} />)}</div></div>}
    <section className="fm-tasks" style={{ height: tasksCollapsed ? 34 : Math.min(taskHeight, maxTaskHeight), minHeight: tasksCollapsed ? 34 : 95 }} aria-label={en ? "Transfers" : "传输任务"}>
      {!tasksCollapsed && <div className="fm-task-resize" role="separator" aria-label={en ? "Resize transfers" : "调整任务区高度"} aria-orientation="horizontal" aria-valuemin={95} aria-valuemax={maxTaskHeight} aria-valuenow={taskHeight} tabIndex={0} onPointerDown={event => { resizing.current = { y: event.clientY, height: taskHeight }; event.currentTarget.setPointerCapture(event.pointerId); }} onPointerMove={event => { if (resizing.current) resizeTasks(resizing.current.height + resizing.current.y - event.clientY); }} onPointerUp={event => { resizing.current = null; event.currentTarget.releasePointerCapture(event.pointerId); }} onLostPointerCapture={() => { resizing.current = null; }} onKeyDown={event => { if (["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) { event.preventDefault(); resizeTasks(event.key === "Home" ? 95 : event.key === "End" ? maxTaskHeight : taskHeight + (event.key === "ArrowUp" ? 20 : -20)); } }} />}
      <header><button title={tasksCollapsed ? en ? "Show transfers" : "展开任务" : en ? "Hide transfers" : "收起任务"} aria-expanded={!tasksCollapsed} onClick={() => setTasksCollapsed(value => !value)}>{tasksCollapsed ? <ChevronUp size={16} /> : <ChevronDown size={16} />}</button><strong>{en ? "Transfers" : "传输任务"}</strong><span>{batches.filter(batch => !terminal(batch)).length} {en ? "active" : "进行中"}</span><button className="fm-clear-completed" disabled={ended || clearingCompleted || !batches.some(batch => ["completed", "completed_with_skips"].includes(batch.phase))} onClick={() => void clearCompleted()}>{en ? "Clear completed" : "清理已完成"}</button><select aria-label={en ? "Transfer filter" : "任务筛选"} value={taskFilter} onChange={event => { setTaskFilter(event.target.value); setTasksCollapsed(false); }}><option value="all">{en ? "All" : "全部"}</option><option value="active">{en ? "Active" : "进行中"}</option><option value="completed">{en ? "Completed" : "已完成"}</option><option value="failed">{en ? "Failed" : "失败"}</option></select></header><div className="fm-task-list" hidden={tasksCollapsed}>{!batches.length ? <div className="fm-empty">{en ? "Select files or folders to start a transfer" : "选择文件或文件夹，开始传输"}</div> : batches.filter(batch => taskFilter === "all" || taskFilter === "active" && !terminal(batch) || taskFilter === "completed" && ["completed", "completed_with_skips"].includes(batch.phase) || taskFilter === "failed" && BigInt(batch.failed) > 0n).map(batch => {
      const total = BigInt(batch.size), confirmed = BigInt(batch.confirmed); const percent = ["completed", "completed_with_skips"].includes(batch.phase) ? 100 : total > 0n ? Number(confirmed * 10000n / total) / 100 : BigInt(batch.total) > 0n ? Number(BigInt(batch.completed) * 10000n / BigInt(batch.total)) / 100 : 0; const resume = batch.phase === "paused" || batch.runner_active === false && ["queued", "reconciling", "interrupted"].includes(batch.phase); const stopping = ["pause_requested", "cancel_requested"].includes(batch.phase);
      const rate = rates[batch.spec.id] || unknownTransferRate;
      const finished = terminal(batch);
      const elapsedMs = batch.active_elapsed_ms && /^\d+$/.test(batch.active_elapsed_ms) ? BigInt(batch.active_elapsed_ms) : null;
      const average = finished && elapsedMs !== null && elapsedMs > 0n ? confirmed * 1000n / elapsedMs : null;
      const shownSpeed = finished ? average : rate.bytesPerSecond;
      return <article className="fm-task" key={batch.spec.id}><span>{batch.spec.direction === "upload" ? <ArrowUp size={17} /> : <ArrowDown size={17} />}{tabs.find(tab => tab.code === String(batch.spec.code))?.name || String(batch.spec.code)}</span><div className="fm-task-progress"><progress max={100} value={percent} /><small>{fileSize(batch.confirmed)} / {fileSize(batch.size)}</small></div><div className="fm-task-speed"><span aria-label={en ? "Transfer speed" : "传输速度"}>{shownSpeed === null ? (finished ? "" : "—") : `${finished ? (en ? "Avg " : "平均 ") : ""}${shownSpeed === 0n ? "<1 B/s" : `${fileSize(shownSpeed.toString())}/s`}`}</span></div><div className="fm-task-elapsed"><span aria-label={en ? "Active transfer time" : "有效传输用时"}>{en ? "Active " : "用时"}{batch.active_elapsed_ms && /^\d+$/.test(batch.active_elapsed_ms) ? remainingTime(BigInt(batch.active_elapsed_ms) / 1000n, en) : "—"}</span></div><div className="fm-task-eta"><span aria-label={en ? "Estimated remaining" : "预计剩余"}>{finished ? "" : rate.etaSeconds === null ? "—" : `${en ? "~" : "剩余"}${remainingTime(rate.etaSeconds, en)}`}</span></div><span>{batch.completed} / {batch.total}</span><span title={batch.message || batch.phase}>{words[batch.phase] || batch.phase}</span><div><button title={en ? "Transfer details" : "传输详情"} onClick={() => setDetails(batch)}>{en ? "Details" : "详情"}</button>{terminal(batch) && batch.intent !== "cancel" && BigInt(batch.failed) > 0n && <button title={en ? "Retry failed files" : "重试失败项目"} disabled={ended || batch.runner_active} onClick={() => void retry(batch.spec.id)}><Play size={16} /></button>}{!terminal(batch) && <><button title={resume ? en ? "Resume" : "继续" : en ? "Pause" : "暂停"} disabled={ended || stopping || batch.intent === "cancel"} onClick={() => void control(batch.spec.id, resume ? "run" : "pause")}>{resume ? <Play size={16} /> : <Pause size={16} />}</button><button title={en ? "Cancel" : "取消"} disabled={ended || batch.intent === "cancel"} onClick={() => void control(batch.spec.id, "cancel")}><X size={16} /></button></>}</div></article>;
    })}</div></section>
    {details && <FileBatchDialog id={details.spec.id} initialConflict={details.phase === "waiting_conflict"} disabled={ended} onClose={() => setDetails(null)} onChange={() => { snapshotEpoch.current++; }} />}
    {dropped && <div className="fm-modal-shade"><div ref={dropForm} role="dialog" aria-modal="true" aria-label={en ? "Send dropped files" : "发送拖入的文件"} className="fm-dialog"><header><strong>{en ? "Send dropped files" : "发送拖入的文件"}</strong></header><p>{dropped.count} {en ? "files or folders" : "个文件或文件夹"} → {tabs.find(tab => tab.code === dropped.code)?.name || dropped.code}</p><p className="fm-drop-destination">{pathLabel(dropped.destination)}</p><ul>{dropped.names.map((name, index) => <li key={index}>{name}</li>)}</ul>{dropped.count > dropped.names.length && <p>…</p>}{error && <p role="alert" className="fm-error">{error}</p>}<footer><button disabled={starting} onClick={() => setDropped(null)}>{en ? "Cancel" : "取消"}</button><button className="primary" disabled={starting || ended} onClick={() => void startDrop()}>{en ? "Send files" : "发送文件"}</button></footer></div></div>}
    {closeDialog && <div className="fm-modal-shade"><div role="dialog" aria-modal="true" aria-label={en ? "Close file transfer" : "关闭文件传输"} className="fm-dialog"><header><strong>{en ? "Transfers are running" : "仍有传输任务"}</strong></header><p>{en ? "Close this window and keep transfers running in the background?" : "关闭窗口后，传输任务将继续在后台运行。"}</p><footer><button onClick={() => setCloseDialog(false)}>{en ? "Stay" : "留在此页"}</button><button className="primary" onClick={() => { closed.current = true; void appWindow.close(); }}>{en ? "Continue in background" : "后台继续"}</button></footer></div></div>}
  </main>;
}
