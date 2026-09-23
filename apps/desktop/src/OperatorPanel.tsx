import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { messages, type Language } from "./i18n";

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

export function OperatorPanel({ language }: { language: Language }) {
  const t = messages[language];
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [devices, setDevices] = useState<ConnectedDevice[]>([]);
  const [selectedCode, setSelectedCode] = useState("");
  const [program, setProgram] = useState("");
  const [argumentsText, setArgumentsText] = useState("");
  const [cwd, setCwd] = useState("");
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
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

  return (
    <section className="card operator-card">
      <div className="section-heading">
        <div>
          <h2>{t.remoteTitle}</h2>
          <p>{t.remoteDescription}</p>
        </div>
      </div>

      {error && <div className="alert" role="alert">{error}</div>}
      <div className="operator-connect">
        <label>
          <span>{t.remoteCode}</span>
          <input value={code} onChange={(event) => setCode(event.target.value)} inputMode="numeric" maxLength={9} />
        </label>
        <label>
          <span>{t.remotePassword}</span>
          <input type="password" value={password} onChange={(event) => setPassword(event.target.value)} />
        </label>
        <button disabled={connecting || code.length !== 9 || !password} onClick={() => void connect()}>
          {connecting ? t.connecting : t.connect}
        </button>
      </div>

      <h3>{t.connectedDevices}</h3>
      {devices.length === 0 ? <p>{t.noConnectedDevices}</p> : (
        <div className="device-tabs">
          {devices.map((device) => (
            <button
              className={selectedCode === device.deviceCode ? "device-tab active" : "device-tab"}
              key={device.deviceCode}
              onClick={() => setSelectedCode(device.deviceCode)}
            >
              {device.deviceCode} · {device.osFamily}
            </button>
          ))}
        </div>
      )}

      {selected && (
        <div className="command-area">
          <h3>{t.commandTitle}</h3>
          <p className="os-reminder">{t.targetOs}: {selected.osReminder}</p>
          <p>{t.nativeCommandHint}</p>
          <div className="command-fields">
            <label><span>{t.program}</span><input value={program} onChange={(event) => setProgram(event.target.value)} /></label>
            <label><span>{t.arguments}</span><textarea value={argumentsText} onChange={(event) => setArgumentsText(event.target.value)} /></label>
            <label><span>{t.cwd}</span><input value={cwd} onChange={(event) => setCwd(event.target.value)} /></label>
          </div>
          <button disabled={submitting || !program.trim()} onClick={() => void runCommand()}>
            {submitting ? t.runningCommand : t.runCommand}
          </button>
        </div>
      )}

      {selected && (
        <div className="command-area">
          <h3>{t.requestOwnership}</h3>
          <p>{t.requestOwnershipDescription}</p>
          <div className="command-fields">
            <label><span>{t.accountName}</span><input value={accountName} onChange={(event) => setAccountName(event.target.value)} /></label>
            <label><span>{t.accountPassword}</span><input type="password" value={accountPassword} onChange={(event) => setAccountPassword(event.target.value)} /></label>
            <label><span>{t.teamId}</span><input value={teamId} onChange={(event) => setTeamId(event.target.value)} /></label>
          </div>
          <button disabled={requestingClaim || !accountName.trim() || !accountPassword} onClick={() => void requestClaim()}>
            {requestingClaim ? t.requestingClaim : t.requestClaim}
          </button>
          {claimResult && <p className="claim-result">{t.claimRequested}: <strong>{claimResult.claimId}</strong></p>}
        </div>
      )}

      {tasks.length > 0 && (
        <div className="task-list">
          <h3>{t.commandHistory}</h3>
          {tasks.map((task) => (
            <article className="task-entry" key={task.id}>
              <div><strong>{task.deviceCode} · {task.program}</strong><span>{task.state}</span></div>
              {task.stdout && <><h4>{t.stdout}</h4><pre>{task.stdout}</pre></>}
              {task.stderr && <><h4>{t.stderr}</h4><pre>{task.stderr}</pre></>}
            </article>
          ))}
        </div>
      )}
    </section>
  );
}
