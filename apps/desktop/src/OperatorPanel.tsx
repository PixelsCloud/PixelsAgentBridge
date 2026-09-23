import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { messages, type Language } from "./i18n";
import type { View } from "./App";

type ConnectedDevice = {
  deviceCode: string;
  osFamily: string;
  osReminder: string;
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
  program: string;
};

type ClaimResult = {
  claimId: string;
  ownerTenantId: string;
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
  const [selectedTaskId, setSelectedTaskId] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [accountName, setAccountName] = useState("");
  const [accountPassword, setAccountPassword] = useState("");
  const [teamId, setTeamId] = useState("");
  const [claimResult, setClaimResult] = useState<ClaimResult | null>(null);
  const [requestingClaim, setRequestingClaim] = useState(false);
  const tasksRef = useRef(tasks);
  const selected = devices.find((device) => device.deviceCode === selectedCode);

  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  useEffect(() => {
    let polling = false;
    let closed = false;
    const poll = async () => {
      if (polling) return;
      polling = true;
      try {
        const pending = tasksRef.current.filter((task) => !task.complete);
        for (const task of pending) {
          try {
            const update = await invoke<TaskUpdate>("operator_task", {
              taskId: task.id,
              stdoutOffset: task.stdoutOffset,
              stderrOffset: task.stderrOffset,
            });
            if (closed) return;
            setTasks((current) => current.map((item) => item.id === task.id ? {
              ...item,
              state: update.state,
              complete: update.complete,
              stdout: item.stdout + update.stdout,
              stderr: item.stderr + update.stderr,
              stdoutOffset: update.stdoutOffset,
              stderrOffset: update.stderrOffset,
            } : item));
          } catch {
            if (closed) return;
            setTasks((current) => current.map((item) => item.id === task.id ? {
              ...item,
              state: "Error",
              complete: true,
            } : item));
          }
        }
      } finally {
        polling = false;
      }
    };
    const timer = window.setInterval(() => void poll(), 500);
    return () => {
      closed = true;
      window.clearInterval(timer);
    };
  }, []);

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
    } catch {
      setError(t.connectFailed);
    } finally {
      setConnecting(false);
    }
  }

  async function runCommand() {
    if (!selected || !program.trim()) return;
    setSubmitting(true);
    setError("");
    try {
      const id = await invoke<string>("operator_run_command", {
        code: selected.deviceCode,
        program: program.trim(),
        args: argumentsText.split(/\r?\n/).filter((line) => line.length > 0),
        cwd: cwd.trim() || null,
      });
      setTasks((current) => [{
        id,
        deviceCode: selected.deviceCode,
        program: program.trim(),
        state: "Accepted",
        complete: false,
        stdout: "",
        stderr: "",
        stdoutOffset: 0,
        stderrOffset: 0,
      }, ...current]);
      setSelectedTaskId(id);
    } catch {
      setError(t.commandFailed);
    } finally {
      setSubmitting(false);
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
  const selectedTask = tasks.find((task) => task.id === selectedTaskId) ?? tasks[0];

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
                  <button className={`device-row ${selectedCode === device.deviceCode ? "active" : ""}`} key={device.deviceCode} onClick={() => setSelectedCode(device.deviceCode)}>
                    <span className="device-avatar">{device.osFamily.slice(0, 1).toUpperCase()}</span>
                    <span><strong>{device.deviceCode}</strong><small>{device.osFamily}</small></span>
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
                <div className="os-banner"><strong>{t.targetOs}: {selected.osFamily}</strong><span>{selected.osReminder}</span></div>
                <p className="form-hint">{t.nativeCommandHint}</p>
                <div className="command-fields">
                  <label><span className="field-label">{t.program}</span><input value={program} onChange={(event) => setProgram(event.target.value)} placeholder="powershell.exe / bash" /></label>
                  <label><span className="field-label">{t.arguments}</span><textarea value={argumentsText} onChange={(event) => setArgumentsText(event.target.value)} placeholder={t.argumentPlaceholder} /></label>
                  <label><span className="field-label">{t.cwd}</span><input value={cwd} onChange={(event) => setCwd(event.target.value)} /></label>
                </div>
                <button className="primary-button" disabled={submitting || !program.trim()} onClick={() => void runCommand()}>{submitting ? t.runningCommand : t.runCommand}<span>→</span></button>
              </>
            ) : <div className="empty-panel"><span>↗</span><strong>{t.selectDevice}</strong><p>{t.selectDeviceHint}</p></div>}
          </section>
        </>
      )}

      {view === "activity" && (
        <section className="surface activity-panel">
          <div className="surface-kicker">01 / {t.commandHistory}</div>
          <div className="surface-topline"><h2>{t.commandHistory}</h2><span className="count-badge">{tasks.length}</span></div>
          {tasks.length === 0 ? <div className="empty-panel"><span>≡</span><strong>{t.noTasks}</strong><p>{t.noTasksHint}</p></div> : (
            <div className="task-layout">
              <div className="task-list">
                {tasks.map((task) => (
                  <button className={`task-row ${selectedTask?.id === task.id ? "active" : ""}`} key={task.id} onClick={() => setSelectedTaskId(task.id)}>
                    <span className="task-icon">›_</span><span><strong>{task.program}</strong><small>{task.deviceCode}</small></span><em>{task.state}</em>
                  </button>
                ))}
              </div>
              <div className="task-output" tabIndex={0}>
                <div className="output-heading"><strong>{selectedTask.program}</strong><span>{selectedTask.state}</span></div>
                <div className="output-meta">{selectedTask.deviceCode}</div>
                <pre>{selectedTask.stdout || (!selectedTask.stderr && t.waitingOutput)}</pre>
                {selectedTask.stderr && <pre className="stderr-output">{selectedTask.stderr}</pre>}
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
