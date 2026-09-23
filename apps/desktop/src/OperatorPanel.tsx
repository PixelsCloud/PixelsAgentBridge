import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { messages, type Language } from "./i18n";
import type { View } from "./App";

type ConnectedDevice = {
  deviceId: string;
  deviceCode: string;
  alias: string;
  osFamily: string;
  osReminder: string;
  connected: boolean;
};

type OperatorBootstrap = {
  devices: ConnectedDevice[];
  tasks: TaskEntry[];
  operations: OperationEntry[];
};

type OperationEntry = {
  id: string;
  deviceCode: string;
  initiatedBy: string;
  kind: string;
  direction: "upload" | "download" | string;
  source: string;
  destination: string;
  overwrite: boolean;
  state: string;
  offset: number;
  size: number;
  startedAtUnixMs: number;
  finishedAtUnixMs: number | null;
  message: string | null;
};

type TaskUpdate = {
  state: string;
  complete: boolean;
  stdout: string;
  stderr: string;
  stdoutOffset: number;
  stderrOffset: number;
};

type TaskEntry = TaskUpdate & {
  id: string;
  deviceCode: string;
  initiatedBy: string;
  program: string;
  args: string[];
  cwd: string | null;
  startedAtUnixMs: number;
};

type ClaimResult = {
  claimId: string;
  ownerTenantId: string;
};

type TransferUpdate = {
  id: string;
  state: "running" | "completed" | "failed" | "cancelled";
  offset: number;
  size: number;
  message: string | null;
};

export function OperatorPanel({ language, view }: { language: Language; view: View }) {
  const t = messages[language];
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [devices, setDevices] = useState<ConnectedDevice[]>([]);
  const [selectedCode, setSelectedCode] = useState("");
  const [program, setProgram] = useState("");
  const [argumentsText, setArgumentsText] = useState("");
  const [cwd, setCwd] = useState("");
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [operations, setOperations] = useState<OperationEntry[]>([]);
  const [selectedActivityId, setSelectedActivityId] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [accountName, setAccountName] = useState("");
  const [accountPassword, setAccountPassword] = useState("");
  const [teamId, setTeamId] = useState("");
  const [claimResult, setClaimResult] = useState<ClaimResult | null>(null);
  const [requestingClaim, setRequestingClaim] = useState(false);
  const [aliasDraft, setAliasDraft] = useState("");
  const [operation, setOperation] = useState<"command" | "transfer">("command");
  const [transferDirection, setTransferDirection] = useState<"upload" | "download">("upload");
  const [transferSource, setTransferSource] = useState("");
  const [transferDestination, setTransferDestination] = useState("");
  const [transferOverwrite, setTransferOverwrite] = useState(false);
  const [transfer, setTransfer] = useState<TransferUpdate | null>(null);
  const [startingTransfer, setStartingTransfer] = useState(false);
  const tasksRef = useRef(tasks);
  const refreshingRef = useRef(new Set<string>());
  const refreshAgainRef = useRef(new Set<string>());
  const selected = devices.find((device) => device.deviceCode === selectedCode);

  async function refreshOperations() {
    const saved = await invoke<OperationEntry[]>("operator_operations");
    setOperations(saved);
  }

  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  async function refreshTask(taskId: string) {
    const task = tasksRef.current.find((item) => item.id === taskId);
    if (!task || task.complete) return;
    if (refreshingRef.current.has(taskId)) {
      refreshAgainRef.current.add(taskId);
      return;
    }
    refreshingRef.current.add(taskId);
    try {
      const update = await invoke<TaskUpdate>("operator_task", {
        taskId,
        stdoutOffset: task.stdoutOffset,
        stderrOffset: task.stderrOffset,
      });
      setTasks((current) => {
        const next = current.map((item) => {
          if (item.id !== taskId || item.stdoutOffset !== task.stdoutOffset || item.stderrOffset !== task.stderrOffset) {
            return item;
          }
          return {
            ...item,
            state: update.state,
            complete: update.complete,
            stdout: item.stdout + update.stdout,
            stderr: item.stderr + update.stderr,
            stdoutOffset: update.stdoutOffset,
            stderrOffset: update.stderrOffset,
          };
        });
        tasksRef.current = next;
        return next;
      });
    } catch {
      setError(t.taskRefreshFailed);
    } finally {
      refreshingRef.current.delete(taskId);
      if (refreshAgainRef.current.delete(taskId)) {
        void refreshTask(taskId);
      }
    }
  }

  useEffect(() => {
    let closed = false;
    let stopTask: (() => void) | undefined;
    let stopDevice: (() => void) | undefined;
    let stopTransfer: (() => void) | undefined;
    void (async () => {
      stopTask = await listen<{ taskId: string }>("operator-task-changed", (event) => {
        void refreshTask(event.payload.taskId);
      });
      if (closed) {
        stopTask();
        return;
      }
      stopDevice = await listen<{ deviceId: string; phase: string }>("operator-device-connection", (event) => {
        setDevices((current) => current.map((device) => device.deviceId === event.payload.deviceId ? {
          ...device,
          connected: event.payload.phase === "connected",
        } : device));
      });
      if (closed) {
        stopDevice();
        return;
      }
      stopTransfer = await listen<TransferUpdate>("operator-transfer", (event) => {
        setTransfer((current) => {
          const next = event.payload;
          if (next.state !== "running" && current?.id === next.id) {
            return { ...next, offset: current.offset, size: current.size };
          }
          return next;
        });
        setOperations((current) => current.map((item) => item.id === event.payload.id ? {
          ...item,
          state: event.payload.state,
          offset: event.payload.state === "running" ? event.payload.offset : item.offset,
          size: event.payload.state === "running" ? event.payload.size : item.size,
          message: event.payload.message,
          finishedAtUnixMs: event.payload.state === "running" ? null : Date.now(),
        } : item));
        if (event.payload.state !== "running") {
          void refreshOperations();
        }
      });
      if (closed) {
        stopTransfer();
        return;
      }
      try {
        const saved = await invoke<OperatorBootstrap>("operator_bootstrap");
        if (closed) return;
        setDevices(saved.devices);
        tasksRef.current = saved.tasks;
        setTasks(saved.tasks);
        setOperations(saved.operations);
        const latest = [...saved.tasks, ...saved.operations].sort((a, b) => b.startedAtUnixMs - a.startedAtUnixMs)[0];
        setSelectedActivityId(latest?.id ?? "");
      } catch {
        if (!closed) setError(t.historyLoadFailed);
      }
    })();
    return () => {
      closed = true;
      stopTask?.();
      stopDevice?.();
      stopTransfer?.();
    };
  }, []);

  useEffect(() => {
    setAliasDraft(selected?.alias ?? "");
  }, [selectedCode, selected?.alias]);

  async function connect() {
    setConnecting(true);
    setError("");
    try {
      const device = await invoke<ConnectedDevice>("operator_connect", {
        code: code.trim(),
        password,
      });
      setDevices((current) => [
        ...current.filter((item) => item.deviceCode !== device.deviceCode),
        device,
      ]);
      setSelectedCode(device.deviceCode);
      setCode("");
      setPassword("");
      setAliasDraft(device.alias);
    } catch {
      setError(t.connectFailed);
    } finally {
      setConnecting(false);
    }
  }

  async function runCommand() {
    if (!selected?.connected || !program.trim()) return;
    setSubmitting(true);
    setError("");
    try {
      const id = await invoke<string>("operator_run_command", {
        code: selected.deviceCode,
        program: program.trim(),
        args: argumentsText.split(/\r?\n/).filter((line) => line.length > 0),
        cwd: cwd.trim() || null,
      });
      const newTask: TaskEntry = {
        id,
        deviceCode: selected.deviceCode,
        initiatedBy: "guest",
        program: program.trim(),
        args: argumentsText.split(/\r?\n/).filter((line) => line.length > 0),
        cwd: cwd.trim() || null,
        startedAtUnixMs: Date.now(),
        state: "Accepted",
        complete: false,
        stdout: "",
        stderr: "",
        stdoutOffset: 0,
        stderrOffset: 0,
      };
      tasksRef.current = [newTask, ...tasksRef.current];
      setTasks(tasksRef.current);
      setSelectedActivityId(id);
      void refreshTask(id);
    } catch {
      setError(t.commandFailed);
    } finally {
      setSubmitting(false);
    }
  }

  async function renameDevice() {
    if (!selected) return;
    try {
      const alias = await invoke<string>("operator_rename_device", {
        code: selected.deviceCode,
        alias: aliasDraft,
      });
      setDevices((current) => current.map((device) => device.deviceCode === selected.deviceCode ? {
        ...device,
        alias,
      } : device));
      setError("");
    } catch {
      setError(t.renameFailed);
    }
  }

  async function startTransfer() {
    if (!selected?.connected || !transferSource.trim() || !transferDestination.trim()) return;
    setStartingTransfer(true);
    setError("");
    try {
      const id = await invoke<string>("operator_start_transfer", {
        code: selected.deviceCode,
        direction: transferDirection,
        source: transferSource.trim(),
        destination: transferDestination.trim(),
        overwrite: transferOverwrite,
      });
      setTransfer((current) => current?.id === id ? current : {
        id,
        state: "running",
        offset: 0,
        size: 0,
        message: null,
      });
      setOperations((current) => [{
        id,
        deviceCode: selected.deviceCode,
        initiatedBy: "guest",
        kind: "file_transfer",
        direction: transferDirection,
        source: transferSource.trim(),
        destination: transferDestination.trim(),
        overwrite: transferOverwrite,
        state: "running",
        offset: 0,
        size: 0,
        startedAtUnixMs: Date.now(),
        finishedAtUnixMs: null,
        message: null,
      }, ...current.filter((item) => item.id !== id)]);
      setSelectedActivityId(id);
    } catch (cause) {
      setError(`${t.transferFailed}: ${String(cause)}`);
    } finally {
      setStartingTransfer(false);
    }
  }

  async function cancelTransfer() {
    if (!transfer || transfer.state !== "running") return;
    try {
      await invoke("operator_cancel_transfer", { id: transfer.id });
    } catch (cause) {
      setError(String(cause));
    }
  }

  async function requestClaim() {
    if (!selected || !accountName.trim() || !accountPassword) return;
    setRequestingClaim(true);
    setClaimResult(null);
    setError("");
    try {
      const result = await invoke<ClaimResult>("operator_claim", {
        code: selected.deviceCode,
        username: accountName.trim(),
        password: accountPassword,
        teamId: teamId.trim() || null,
      });
      setClaimResult(result);
      setAccountPassword("");
    } catch {
      setError(t.claimRequestFailed);
    } finally {
      setRequestingClaim(false);
    }
  }

  const connectionForm = (
    <div className="connect-form">
      <label><span className="field-label">{t.remoteCode}</span><input value={code} onChange={(event) => setCode(event.target.value)} inputMode="numeric" maxLength={9} placeholder="000 000 000" /></label>
      <label><span className="field-label">{t.remotePassword}</span><input type="password" value={password} onChange={(event) => setPassword(event.target.value)} placeholder="••••••••••••" /></label>
      <button className="primary-button" disabled={connecting || code.length !== 9 || !password} onClick={() => void connect()}>{connecting ? t.connecting : t.connect}<span>→</span></button>
    </div>
  );
  const activity = [...tasks.map((task) => ({ kind: "command" as const, item: task })),
    ...operations.map((item) => ({ kind: "operation" as const, item }))]
    .sort((a, b) => b.item.startedAtUnixMs - a.item.startedAtUnixMs);
  const selectedActivity = activity.find(({ item }) => item.id === selectedActivityId) ?? activity[0];
  const selectedTask = selectedActivity?.kind === "command" ? selectedActivity.item : null;
  const selectedOperation = selectedActivity?.kind === "operation" ? selectedActivity.item : null;
  const formatTime = (value: number) => new Date(value).toLocaleString(language);
  const operationLabel = (item: OperationEntry) => item.kind === "screenshot"
    ? t.screenshot
    : item.direction === "upload" ? t.upload : t.download;
  const formatActor = (actor: string) => actor === "guest" ? t.guestOperator : actor;

  return (
    <>
      {view === "remote" && (
        <>
          <section className="surface remote-list">
            <div className="surface-kicker">01 / {t.remoteTitle}</div>
            <h2>{t.connect}</h2>
            <p>{t.remoteDescription}</p>
            {error && <div className="inline-error" role="alert">{error}</div>}
            {connectionForm}
            <div className="section-separator" />
            <div className="list-heading"><h3>{t.connectedDevices}</h3><span>{devices.length}</span></div>
            {devices.length === 0 ? <div className="empty-list">{t.noConnectedDevices}</div> : (
              <div className="device-list">
                {devices.map((device) => (
                  <button className={`device-row ${selectedCode === device.deviceCode ? "active" : ""}`} key={device.deviceCode} onClick={() => {
                    setSelectedCode(device.deviceCode);
                    if (!device.connected) setCode(device.deviceCode);
                  }}>
                    <span className="device-avatar">{device.osFamily.slice(0, 1).toUpperCase()}</span>
                    <span><strong>{device.alias || device.deviceCode}</strong><small>{device.deviceCode} · {device.osFamily} · {device.connected ? t.connected : t.reconnectRequired}</small></span>
                    <span className="device-arrow">›</span>
                  </button>
                ))}
              </div>
            )}
          </section>
          <section className="surface remote-command">
            <div className="surface-kicker">02 / {t.commandTitle}</div>
            <div className="surface-topline"><h2>{t.commandTitle}</h2>{selected && <span className="target-badge">{selected.deviceCode}</span>}</div>
            {selected ? (
              <>
                <div className="device-alias-row">
                  <input value={aliasDraft} onChange={(event) => setAliasDraft(event.target.value)} maxLength={64} placeholder={t.deviceAlias} />
                  <button className="quiet-button" onClick={() => void renameDevice()}>{t.saveName}</button>
                </div>
                <div className="os-banner"><strong>{t.targetOs}: {selected.osFamily}</strong><span>{selected.osReminder}</span></div>
                {!selected.connected && <p className="form-hint">{t.reconnectHint}</p>}
                <div className="operation-tabs">
                  <button className={operation === "command" ? "active" : ""} onClick={() => setOperation("command")}>{t.commandTitle}</button>
                  <button className={operation === "transfer" ? "active" : ""} onClick={() => setOperation("transfer")}>{t.fileTransfer}</button>
                </div>
                {operation === "command" ? <>
                  <p className="form-hint">{t.nativeCommandHint}</p>
                  <div className="command-fields">
                    <label><span className="field-label">{t.program}</span><input value={program} onChange={(event) => setProgram(event.target.value)} placeholder="powershell.exe / bash" /></label>
                    <label><span className="field-label">{t.arguments}</span><textarea value={argumentsText} onChange={(event) => setArgumentsText(event.target.value)} placeholder={t.argumentPlaceholder} /></label>
                    <label><span className="field-label">{t.cwd}</span><input value={cwd} onChange={(event) => setCwd(event.target.value)} /></label>
                  </div>
                  <button className="primary-button" disabled={submitting || !selected.connected || !program.trim()} onClick={() => void runCommand()}>{submitting ? t.runningCommand : t.runCommand}<span>→</span></button>
                </> : <>
                  <div className="transfer-directions">
                    <button className={transferDirection === "upload" ? "active" : ""} onClick={() => setTransferDirection("upload")}>{t.upload}</button>
                    <button className={transferDirection === "download" ? "active" : ""} onClick={() => setTransferDirection("download")}>{t.download}</button>
                  </div>
                  <div className="command-fields">
                    <label><span className="field-label">{transferDirection === "upload" ? t.localSource : t.remoteSource}</span><input value={transferSource} onChange={(event) => setTransferSource(event.target.value)} /></label>
                    <label><span className="field-label">{transferDirection === "upload" ? t.remoteDestination : t.localDestination}</span><input value={transferDestination} onChange={(event) => setTransferDestination(event.target.value)} /></label>
                  </div>
                  <label className="check-row"><input type="checkbox" checked={transferOverwrite} onChange={(event) => setTransferOverwrite(event.target.checked)} />{t.overwriteExisting}</label>
                  <button className="primary-button" disabled={startingTransfer || transfer?.state === "running" || !selected.connected || !transferSource.trim() || !transferDestination.trim()} onClick={() => void startTransfer()}>{startingTransfer ? t.startingTransfer : t.startTransfer}<span>→</span></button>
                  {transfer && <div className="transfer-status">
                    <div><strong>{t.transferStates[transfer.state]}</strong><span>{transfer.size ? `${Math.round(transfer.offset / transfer.size * 100)}% · ${transfer.offset} / ${transfer.size} B` : ""}</span></div>
                    <progress value={transfer.offset} max={Math.max(transfer.size, 1)} />
                    {transfer.message && <small>{transfer.message}</small>}
                    {transfer.state === "running" && <button className="quiet-button" onClick={() => void cancelTransfer()}>{t.cancelTransfer}</button>}
                  </div>}
                </>}
              </>
            ) : <div className="empty-panel"><span>↗</span><strong>{t.selectDevice}</strong><p>{t.selectDeviceHint}</p></div>}
          </section>
        </>
      )}

      {view === "activity" && (
        <section className="surface activity-panel">
          <div className="surface-kicker">01 / {t.operationHistory}</div>
          <div className="surface-topline"><h2>{t.operationHistory}</h2><span className="count-badge">{activity.length}</span></div>
          {activity.length === 0 ? <div className="empty-panel"><span>≡</span><strong>{t.noTasks}</strong><p>{t.noTasksHint}</p></div> : (
            <div className="task-layout">
              <div className="task-list">
                {activity.map(({ kind, item }) => (
                  <button className={`task-row ${selectedActivity?.item.id === item.id ? "active" : ""}`} key={item.id} onClick={() => setSelectedActivityId(item.id)}>
                    <span className="task-icon">{kind === "command" ? "›_" : item.kind === "screenshot" ? "▣" : item.direction === "upload" ? "↑" : "↓"}</span>
                    <span><strong>{kind === "command" ? item.program : operationLabel(item)}</strong><small>{item.deviceCode} · {formatTime(item.startedAtUnixMs)}</small></span>
                    <em>{kind === "command" ? item.state : t.transferStates[item.state as keyof typeof t.transferStates] ?? item.state}</em>
                  </button>
                ))}
              </div>
              <div className="task-output" tabIndex={0}>
                {selectedTask && <>
                  <div className="output-heading"><strong>{selectedTask.program}</strong><span>{selectedTask.state}</span></div>
                  <div className="output-meta">{selectedTask.deviceCode} · {t.commandDirection} · {formatTime(selectedTask.startedAtUnixMs)}</div>
                  <div className="command-audit"><span>{t.initiatedBy}</span><code>{formatActor(selectedTask.initiatedBy)}</code></div>
                  <div className="command-audit"><span>{t.commandArguments}</span><code>{selectedTask.args.length ? selectedTask.args.join(" · ") : "—"}</code></div>
                  {selectedTask.cwd && <div className="command-audit"><span>{t.cwd}</span><code>{selectedTask.cwd}</code></div>}
                  <pre>{selectedTask.stdout || (!selectedTask.stderr && t.waitingOutput)}</pre>
                  {selectedTask.stderr && <pre className="stderr-output">{selectedTask.stderr}</pre>}
                  {!selectedTask.complete && <button className="quiet-button" onClick={() => void refreshTask(selectedTask.id)}>{t.loadMoreOutput}</button>}
                </>}
                {selectedOperation && <>
                  <div className="output-heading"><strong>{operationLabel(selectedOperation)}</strong><span>{t.transferStates[selectedOperation.state as keyof typeof t.transferStates] ?? selectedOperation.state}</span></div>
                  <div className="output-meta">{selectedOperation.deviceCode} · {formatTime(selectedOperation.startedAtUnixMs)}</div>
                  <div className="operation-details">
                    <div><span>{t.initiatedBy}</span><strong>{formatActor(selectedOperation.initiatedBy)}</strong></div>
                    <div><span>{t.direction}</span><strong>{selectedOperation.direction === "upload" ? t.uploadDirection : t.downloadDirection}</strong></div>
                    <div><span>{t.sourcePath}</span><strong>{selectedOperation.source}</strong></div>
                    <div><span>{t.destinationPath}</span><strong>{selectedOperation.destination}</strong></div>
                    <div><span>{t.progress}</span><strong>{selectedOperation.offset.toLocaleString()} / {selectedOperation.size.toLocaleString()} B</strong></div>
                    <div><span>{t.overwrite}</span><strong>{selectedOperation.overwrite ? t.yes : t.no}</strong></div>
                    {selectedOperation.finishedAtUnixMs && <div><span>{t.finishedAt}</span><strong>{formatTime(selectedOperation.finishedAtUnixMs)}</strong></div>}
                    {selectedOperation.message && <div><span>{t.result}</span><strong>{selectedOperation.message}</strong></div>}
                  </div>
                </>}
              </div>
            </div>
          )}
        </section>
      )}

      {view === "ownership" && (
        <section className="surface ownership-request">
          <div className="surface-kicker">01 / {t.requestOwnership}</div>
          <h2>{t.requestOwnership}</h2>
          <p>{t.requestOwnershipDescription}</p>
          {error && <div className="inline-error" role="alert">{error}</div>}
          {selected ? (
            <>
              <div className="selected-target">{t.connectedDevices}<strong>{selected.deviceCode}</strong></div>
              <div className="command-fields">
                <label><span className="field-label">{t.accountName}</span><input value={accountName} onChange={(event) => setAccountName(event.target.value)} /></label>
                <label><span className="field-label">{t.accountPassword}</span><input type="password" value={accountPassword} onChange={(event) => setAccountPassword(event.target.value)} /></label>
                <label><span className="field-label">{t.teamId}</span><input value={teamId} onChange={(event) => setTeamId(event.target.value)} /></label>
              </div>
              <button className="primary-button" disabled={requestingClaim || !accountName.trim() || !accountPassword} onClick={() => void requestClaim()}>{requestingClaim ? t.requestingClaim : t.requestClaim}<span>→</span></button>
              {claimResult && <div className="claim-result">{t.claimRequested}<strong>{claimResult.claimId}</strong></div>}
            </>
          ) : <div className="empty-panel"><span>◇</span><strong>{t.selectDevice}</strong><p>{t.selectDeviceHint}</p></div>}
        </section>
      )}
    </>
  );
}
