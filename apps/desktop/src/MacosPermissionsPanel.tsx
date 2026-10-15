import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Dropdown, Modal, Space, Tag, Tooltip, theme } from "antd";
import { AppWindow, Copy, Ellipsis, FolderOpen, RefreshCw, Server } from "lucide-react";
import type { CSSProperties } from "react";
import { macosPermissionText } from "./macosPermissionText";
import "./macosPermissions.css";
import type { Language } from "./i18n";

type Permissions = { screenRecording: boolean; accessibility: boolean };
type Target = "desktop" | "executor";
type Program = { target: Target; path: string; exists: boolean };
type DiskReport = { state: "normal" | "restricted" | "failed"; folders: { folder: string; state: "normal" | "restricted" | "failed"; reason: string | null }[]; checkedAtUnixMs: number; source: string };
type DiskStatus = { checking: boolean; report: DiskReport | null; error?: string };
let startupRequest: Promise<void> | undefined;
export function requestMacosPermissionsAtStartup(): Promise<void> {
  return startupRequest ??= (async () => {
    const permissions = await invoke<Permissions | null>("macos_permissions");
    if (!permissions) return;
    // Request in order, from the foreground app. Checks/polling never prompt.
    if (!permissions.screenRecording) await invoke("request_macos_permission", { permission: "screen", automatic: true });
    if (!permissions.accessibility) await invoke("request_macos_permission", { permission: "accessibility", automatic: true });
  })();
}
const missingCopy = {
  "zh-CN": { title: "权限待开启", screen: "屏幕录制当前不可用，无法截图。", input: "辅助功能当前不可用，无法控制桌面。" },
  "zh-TW": { title: "權限待開啟", screen: "螢幕錄製目前無法使用，無法擷取畫面。", input: "輔助使用目前無法使用，無法控制桌面。" },
  en: { title: "Permissions needed", screen: "Screen Recording is unavailable; screenshots cannot be taken.", input: "Accessibility is unavailable; desktop control cannot be used." },
};
function useMacosPermissions() {
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [error, setError] = useState("");
  const refresh = useCallback(() => invoke<Permissions | null>("macos_permissions").then((value) => { setPermissions(value); setError(""); return true; }).catch((e) => { setError(String(e)); return false; }), []);
  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void refresh(); }, 2000);
    return () => { window.removeEventListener("focus", onFocus); document.removeEventListener("visibilitychange", onFocus); window.clearInterval(timer); };
  }, [refresh]);
  return { permissions, setPermissions, error, setError, refresh };
}

export function MacosPermissionStatus({ language, onOpenSettings }: { language: Language; onOpenSettings: () => void }) {
  const { permissions } = useMacosPermissions();
  const { token } = theme.useToken();
  if (!permissions || (permissions.screenRecording && permissions.accessibility)) return null;
  const text = missingCopy[language];
  const detail = [!permissions.screenRecording && text.screen, !permissions.accessibility && text.input].filter(Boolean).join(" ");
  return <Tooltip title={detail}>
    <button type="button" className="sidebar-status sidebar-permission-status" style={{ color: token.colorWarningText }} onClick={onOpenSettings}>
      <span className="status-dot" aria-hidden="true" />
      <span>{text.title}</span>
    </button>
  </Tooltip>;
}

export function MacosPermissionsPanel({ language }: { language: Language }) {
  const { permissions, error: queryError, refresh } = useMacosPermissions();
  const [error, setError] = useState("");
  const { token } = theme.useToken();
  const [busy, setBusy] = useState<string | null>(null);
  const operationBusy = useRef(false);
  const [programs, setPrograms] = useState<Program[]>([]);
  const [feedback, setFeedback] = useState("");
  const [guide, setGuide] = useState<{ target: Target; permission: "screen" | "accessibility" | "full_disk" } | null>(null);
  const [confirmation, setConfirmation] = useState<"app" | "executor" | null>(null);
  const [disk, setDisk] = useState<Record<Target, DiskStatus>>({ desktop: { checking: false, report: null }, executor: { checking: false, report: null } });
  const mounted = useRef(true);
  const diskBusy = useRef({ desktop: false, executor: false });
  const queuedCheck = useRef({ desktop: false, executor: false });
  const pendingReturn = useRef<Target | null>(null);
  const checkDisk = useCallback(async (target: Target, queueIfBusy = false): Promise<void> => {
    if (diskBusy.current[target]) { if (queueIfBusy) queuedCheck.current[target] = true; return; }
    diskBusy.current[target] = true;
    if (mounted.current) setDisk(old => ({ ...old, [target]: { ...old[target], checking: true } }));
    try {
      const report = await invoke<DiskReport>("macos_file_access_check", { target });
      if (mounted.current) setDisk(old => ({ ...old, [target]: { checking: false, report } }));
    } catch (e) {
      if (mounted.current) setDisk(old => ({ ...old, [target]: { checking: false, report: null, error: String(e) } }));
    } finally {
      diskBusy.current[target] = false;
      if (queuedCheck.current[target] && mounted.current) { queuedCheck.current[target] = false; void checkDisk(target); }
    }
  }, []);
  const text = macosPermissionText[language];
  const isMac = permissions !== null;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    if (!isMac) return;
    void checkDisk("desktop"); void checkDisk("executor");
    const onReturn = () => {
      const target = pendingReturn.current;
      if (target && document.visibilityState === "visible") { pendingReturn.current = null; void checkDisk(target, true); }
    };
    window.addEventListener("focus", onReturn);
    document.addEventListener("visibilitychange", onReturn);
    return () => { window.removeEventListener("focus", onReturn); document.removeEventListener("visibilitychange", onReturn); };
  }, [isMac, checkDisk]);
  useEffect(() => {
    if (!isMac) return;
    let active = true;
    void invoke<Program[]>("macos_permission_targets").then(value => {
      if (active) setPrograms(value);
    }).catch(e => { if (active) setError(`${text.loadError} ${String(e)}`); });
    return () => { active = false; };
  }, [isMac, text.loadError]);
  if (!permissions) return null;
  async function act(name: string, action: () => Promise<void>, success = "") {
    if (operationBusy.current) return;
    operationBusy.current = true;
    setBusy(name); setError(""); setFeedback("");
    try { await action(); setFeedback(success); }
    catch (e) { setError(String(e)); }
    finally { operationBusy.current = false; setBusy(null); }
  }
  function request(target: Target, permission: "screen" | "accessibility" | "full_disk") {
    void act("request", async () => {
      if (permission !== "full_disk") await invoke("request_macos_permission", { permission, automatic: false });
      if (permission === "full_disk") pendingReturn.current = target;
      await invoke("open_macos_permission_settings", { permission });
      if (!await refresh()) throw new Error(text.checkFailed);
    }, permission === "full_disk" ? `${text.enableTarget}${target === "desktop" ? text.desktop : text.executor}` : "");
  }
  async function restart() {
    if (!confirmation) return;
    const target = confirmation;
    await act("restart", () => invoke(target === "app" ? "restart_macos_permission_processes" : "restart_macos_executor_for_permissions"), target === "executor" ? text.serviceRestarted : "");
    setConfirmation(null);
  }
  const disabled = busy !== null;
  const program = programs.find(p => p.target === guide?.target);
  const guideTitle = guide && `${guide.target === "desktop" ? text.desktop : text.executor} · ${guide.permission === "screen" ? text.screen : guide.permission === "accessibility" ? text.input : text.disk}`;
  function row(target: Target, permission: "screen" | "accessibility", title: string, allowed: boolean) {
    return <div className="macos-permission-row" role="group" aria-label={title}>
      <span>{title}</span>
      <div className="macos-permission-row-status">
        <Tag color={allowed ? "green" : "orange"}>{allowed ? text.normal : text.missing}</Tag>
        {!allowed && <Button size="small" disabled={disabled} onClick={() => request(target, permission)}>{text.request}</Button>}
      </div>
    </div>;
  }
  const errors = (error || queryError) && <Alert type="error" showIcon title={error || queryError} />;
  function diskRow(target: Target) {
    const status = disk[target];
    const state = status.report?.state;
    const label = status.checking ? text.checking : status.error || state === "failed" ? text.failed : state === "normal" ? text.normal : state === "restricted" ? text.restricted : text.notChecked;
    const detail = status.error || status.report?.folders.map(f => `${f.folder}: ${f.state === "normal" ? text.normal : f.state === "restricted" ? text.restricted : f.reason === "timeout" ? text.timedOut : text.failed}`).join(" · ") || text.fileNote;
    return <div className="macos-permission-row" role="group" aria-label={text.fileAccess}>
      <span>{text.fileAccess}</span><div className="macos-permission-row-status">
        <Tooltip title={detail}><Tag color={!status.checking && state === "normal" ? "green" : !status.checking && state === "restricted" ? "orange" : undefined}>{label}</Tag></Tooltip>
        {!status.checking && state !== "normal" && <Button size="small" disabled={disabled || !programs.find(p => p.target === target)?.exists} onClick={() => request(target, "full_disk")}>{text.configure}</Button>}
      </div>
    </div>;
  }
  function more(target: Target) {
    return <Dropdown trigger={["click"]} menu={{ items: [{ key: "manage", label: text.manageDisk }, { key: "add", label: text.notListed }], onClick: ({ key }) => { if (key === "manage") request(target, "full_disk"); else { setFeedback(""); setGuide({ target, permission: "full_disk" }); } } }}>
      <Button type="text" size="small" icon={<Ellipsis size={16} />} aria-label={`${target === "desktop" ? text.desktop : text.executor} · ${text.more}`} disabled={disabled} />
    </Dropdown>;
  }
  return <section className="macos-permissions-panel" aria-label={text.title} style={{ "--permission-border": token.colorBorderSecondary, "--permission-surface": token.colorFillQuaternary, "--permission-text-secondary": token.colorTextSecondary } as CSSProperties}>
    <div className="macos-permission-header"><h2>{text.title}</h2><Button type="text" icon={<RefreshCw size={15} />} loading={busy === "refresh"} disabled={disabled} onClick={() => void act("refresh", async () => { const ok = await refresh(); await Promise.all([checkDisk("desktop", true), checkDisk("executor", true)]); if (!ok) throw new Error(text.checkFailed); })}>{text.refresh}</Button></div>
    <section className="macos-permission-program" aria-label={text.desktop}>
      <h3><AppWindow size={18} /><span>{text.desktop}</span>{more("desktop")}</h3>
      {row("desktop", "screen", text.screen, permissions.screenRecording)}
      {row("desktop", "accessibility", text.input, permissions.accessibility)}
      {diskRow("desktop")}
    </section>
    <section className="macos-permission-program" aria-label={text.executor}>
      <h3><Server size={18} /><span>{text.executor}</span>{more("executor")}</h3><p className="macos-permission-note">{text.executorNote}</p>
      {diskRow("executor")}
    </section>
    <p className="macos-permission-note">{text.fileNote}</p>
    {!guide && !confirmation && <>{feedback && <span role="status">{feedback}</span>}{errors}</>}
    <Modal open={guide !== null} title={guideTitle} footer={<Button onClick={() => { setGuide(null); void refresh(); }} disabled={disabled}>{text.done}</Button>} onCancel={() => { if (!disabled) { setGuide(null); void refresh(); } }} maskClosable={!disabled} keyboard={!disabled}>
      <div className="macos-permission-help">
        <p>{guide?.permission === "full_disk" ? text.diskHelp : guide?.permission === "screen" ? text.screenHelp : text.inputHelp}</p>
        {guide?.permission === "full_disk" && <>
          <code>{program?.path}</code>
          {program && !program.exists && <p role="alert">{text.notInstalled}</p>}
          <Space wrap>
            <Button icon={<FolderOpen size={14} />} disabled={disabled || !program?.exists} onClick={() => void act("reveal", () => invoke("reveal_macos_permission_target", { target: guide.target }))}>{text.reveal}</Button>
            <Button icon={<Copy size={14} />} disabled={disabled || !program} onClick={() => void act("copy", () => navigator.clipboard.writeText(program!.path), text.copied)}>{text.copyPath}</Button>
          </Space>
        </>}
        <Button type="primary" disabled={disabled} loading={busy === "request" || busy === "open"} onClick={() => void act("open", () => invoke("open_macos_permission_settings", { permission: guide!.permission }))}>{text.open}</Button>
        <details><summary>{text.repair}</summary><p>{text.repairBody}</p><Space wrap>
          <Button disabled={disabled} onClick={() => setConfirmation("app")}>{text.restart}</Button>
          <Button disabled={disabled} onClick={() => setConfirmation("executor")}>{text.restartService}</Button>
        </Space></details>
        {!confirmation && <>{feedback && <span role="status">{feedback}</span>}{errors}</>}
      </div>
    </Modal>
    <Modal open={confirmation !== null} title={confirmation === "executor" ? text.serviceRestartTitle : text.restartTitle} okText={text.confirmRestart} cancelText={text.cancel} confirmLoading={disabled} cancelButtonProps={{ disabled }} maskClosable={!disabled} keyboard={!disabled} onOk={() => void restart()} onCancel={() => { if (!disabled) setConfirmation(null); }}>
      <p>{confirmation === "executor" ? text.serviceRestartBody : text.restartBody}</p>
      {errors}
    </Modal>
  </section>;
}
