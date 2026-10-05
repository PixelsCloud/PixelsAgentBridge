import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Space, Tag, Tooltip, theme } from "antd";
import type { Language } from "./i18n";

type Permissions = { screenRecording: boolean; accessibility: boolean };
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
const copy = {
  "zh-CN": { title: "macOS 权限", screen: "屏幕录制", input: "辅助功能", allowed: "可用", missing: "当前不可用", request: "申请授权", open: "打开设置", refresh: "重新检查", restart: "重启应用和辅助进程", hint: "未授权时，请在系统设置中开启权限。已经开启但仍不可用时，可重启应用和辅助进程使授权生效；重启会中断当前桌面控制。升级更换签名后，可能需要在设置中移除旧条目，再为当前应用授权。" },
  "zh-TW": { title: "macOS 權限", screen: "螢幕錄製", input: "輔助使用", allowed: "可用", missing: "目前無法使用", request: "申請授權", open: "開啟設定", refresh: "重新檢查", restart: "重新啟動應用程式及輔助程序", hint: "尚未授權時，請在系統設定中開啟權限。已開啟但仍無法使用時，可重新啟動應用程式及輔助程序使授權生效；重新啟動會中斷目前的桌面控制。更新更換簽章後，可能需要在設定中移除舊項目，再為目前應用程式授權。" },
  en: { title: "macOS permissions", screen: "Screen Recording", input: "Accessibility", allowed: "Available", missing: "Currently unavailable", request: "Request access", open: "Open Settings", refresh: "Check again", restart: "Restart app and helper", hint: "Enable missing permissions in System Settings. If already enabled, restart the app and helper to apply access; this interrupts desktop control. If an update changed the signing identity, you may need to remove the old entry in Settings and authorize the current app." },
};
const missingCopy = {
  "zh-CN": { title: "权限待开启", screen: "屏幕录制当前不可用，无法截图。", input: "辅助功能当前不可用，无法控制桌面。" },
  "zh-TW": { title: "權限待開啟", screen: "螢幕錄製目前無法使用，無法擷取畫面。", input: "輔助使用目前無法使用，無法控制桌面。" },
  en: { title: "Permissions needed", screen: "Screen Recording is unavailable; screenshots cannot be taken.", input: "Accessibility is unavailable; desktop control cannot be used." },
};
function useMacosPermissions() {
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [error, setError] = useState("");
  const refresh = useCallback(() => invoke<Permissions | null>("macos_permissions").then((value) => { setPermissions(value); setError(""); }).catch((e) => setError(String(e))), []);
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
  const { permissions, setPermissions, error, setError, refresh } = useMacosPermissions();
  const [requesting, setRequesting] = useState<string | null>(null);
  const text = copy[language];
  if (!permissions) return null;
  async function open(permission: string) {
    setError("");
    try { await invoke("open_macos_permission_settings", { permission }); }
    catch (e) { setError(String(e)); }
  }
  async function request(permission: string) {
    setRequesting(permission);
    setError("");
    try { setPermissions(await invoke<Permissions | null>("request_macos_permission", { permission, automatic: false })); }
    catch (e) { setError(String(e)); }
    finally { setRequesting(null); }
  }
  async function restart() {
    setRequesting("restart");
    setError("");
    try { await invoke("restart_macos_permission_processes"); }
    catch (e) { setError(String(e)); }
    finally { setRequesting(null); }
  }
  return <div style={{ marginTop: 24 }}>
    <h2>{text.title}</h2><p>{text.hint}</p>
    <Space orientation="vertical" style={{ marginTop: 12 }}>
      <Space wrap><span>{text.screen}</span><Tag color={permissions.screenRecording ? "green" : "orange"}>{permissions.screenRecording ? text.allowed : text.missing}</Tag>{!permissions.screenRecording && <Button type="primary" loading={requesting === "screen"} disabled={requesting !== null} onClick={() => void request("screen")}>{text.request}</Button>}<Button onClick={() => void open("screen")}>{text.open}</Button></Space>
      <Space wrap><span>{text.input}</span><Tag color={permissions.accessibility ? "green" : "orange"}>{permissions.accessibility ? text.allowed : text.missing}</Tag>{!permissions.accessibility && <Button type="primary" loading={requesting === "accessibility"} disabled={requesting !== null} onClick={() => void request("accessibility")}>{text.request}</Button>}<Button onClick={() => void open("accessibility")}>{text.open}</Button></Space>
      <Button onClick={() => void refresh()}>{text.refresh}</Button>
      <Button loading={requesting === "restart"} disabled={requesting !== null} onClick={() => void restart()}>{text.restart}</Button>
    </Space>
    {error && <p role="alert">{error}</p>}
  </div>;
}
