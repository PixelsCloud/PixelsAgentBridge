import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Space, Tag } from "antd";
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
  "zh-CN": { title: "macOS 权限", screen: "屏幕录制", input: "辅助功能", allowed: "已授权", missing: "未授权", request: "申请授权", open: "打开设置", refresh: "重新检查", hint: "应用会主动申请截图和桌面控制权限，请在系统提示中允许。若此前拒绝，请打开设置启用；授权后如仍不可用，请重新启动应用和后台辅助进程（或重新登录）。" },
  "zh-TW": { title: "macOS 權限", screen: "螢幕錄製", input: "輔助使用", allowed: "已授權", missing: "未授權", request: "申請授權", open: "開啟設定", refresh: "重新檢查", hint: "應用程式會主動申請擷取畫面及桌面控制權限，請在系統提示中允許。若先前拒絕，請開啟設定啟用；授權後若仍無法使用，請重新啟動應用程式及背景輔助程序（或重新登入）。" },
  en: { title: "macOS permissions", screen: "Screen Recording", input: "Accessibility", allowed: "Allowed", missing: "Not allowed", request: "Request access", open: "Open Settings", refresh: "Check again", hint: "The app requests capture and desktop control access. Allow it in the system prompt. If previously denied, enable it in Settings. If access is still unavailable, restart the app and background helper (or log out and back in)." },
};
const missingCopy = {
  "zh-CN": { title: "请开启所需权限", screen: "屏幕录制未授权，无法截图。", input: "辅助功能未授权，无法控制桌面。" },
  "zh-TW": { title: "請開啟所需權限", screen: "螢幕錄製未授權，無法擷取畫面。", input: "輔助使用未授權，無法控制桌面。" },
  en: { title: "Permissions required", screen: "Screen Recording is required for screenshots.", input: "Accessibility is required for desktop control." },
};
export function MacosPermissionsPanel({ language, persistent = false }: { language: Language; persistent?: boolean }) {
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [error, setError] = useState("");
  const [requesting, setRequesting] = useState<string | null>(null);
  const text = copy[language];
  const refresh = useCallback(() => invoke<Permissions | null>("macos_permissions").then(setPermissions).catch((e) => setError(String(e))), []);
  useEffect(() => {
    void refresh();
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void refresh(); }, 2000);
    return () => { window.removeEventListener("focus", onFocus); document.removeEventListener("visibilitychange", onFocus); window.clearInterval(timer); };
  }, [refresh]);
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
  if (persistent) {
    if (permissions.screenRecording && permissions.accessibility) return null;
    const notice = missingCopy[language];
    return <Alert className="macos-permission-notice" type="warning" showIcon
      title={notice.title}
      description={<Space orientation="vertical" size={4}>
        {([ ["screen", permissions.screenRecording, notice.screen], ["accessibility", permissions.accessibility, notice.input] ] as const)
          .filter(([, allowed]) => !allowed).map(([permission, , hint]) => <Space key={permission} wrap size={8}>
            <span>{hint}</span>
            <Button size="small" loading={requesting === permission} disabled={requesting !== null} onClick={() => void request(permission)}>{text.request}</Button>
            <Button size="small" type="link" onClick={() => void open(permission)}>{text.open}</Button>
          </Space>)}
        {error && <span role="alert">{error}</span>}
      </Space>} />;
  }
  return <div style={{ marginTop: 24 }}>
    <h2>{text.title}</h2><p>{text.hint}</p>
    <Space direction="vertical" style={{ marginTop: 12 }}>
      <Space wrap><span>{text.screen}</span><Tag color={permissions.screenRecording ? "green" : "orange"}>{permissions.screenRecording ? text.allowed : text.missing}</Tag>{!permissions.screenRecording && <Button type="primary" loading={requesting === "screen"} disabled={requesting !== null} onClick={() => void request("screen")}>{text.request}</Button>}<Button onClick={() => void open("screen")}>{text.open}</Button></Space>
      <Space wrap><span>{text.input}</span><Tag color={permissions.accessibility ? "green" : "orange"}>{permissions.accessibility ? text.allowed : text.missing}</Tag>{!permissions.accessibility && <Button type="primary" loading={requesting === "accessibility"} disabled={requesting !== null} onClick={() => void request("accessibility")}>{text.request}</Button>}<Button onClick={() => void open("accessibility")}>{text.open}</Button></Space>
      <Button onClick={() => void refresh()}>{text.refresh}</Button>
    </Space>
    {error && <p role="alert">{error}</p>}
  </div>;
}
