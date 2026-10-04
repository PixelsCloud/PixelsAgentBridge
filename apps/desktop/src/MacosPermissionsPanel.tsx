import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Space, Tag } from "antd";
import type { Language } from "./i18n";

type Permissions = { screenRecording: boolean; accessibility: boolean };
const copy = {
  "zh-CN": { title: "macOS 权限", screen: "屏幕录制", input: "辅助功能", allowed: "已授权", missing: "未授权", open: "打开设置", refresh: "重新检查", hint: "允许 Pixels Agent Bridge 截图和控制桌面。屏幕录制授权后请重新启动应用；后台辅助进程可在重新登录后生效。" },
  "zh-TW": { title: "macOS 權限", screen: "螢幕錄製", input: "輔助使用", allowed: "已授權", missing: "未授權", open: "開啟設定", refresh: "重新檢查", hint: "允許 Pixels Agent Bridge 擷取及控制桌面。螢幕錄製授權後請重新啟動應用程式；背景輔助程序可在重新登入後生效。" },
  en: { title: "macOS permissions", screen: "Screen Recording", input: "Accessibility", allowed: "Allowed", missing: "Not allowed", open: "Open Settings", refresh: "Check again", hint: "Allow Pixels Agent Bridge to capture and control the desktop. Restart the app after granting Screen Recording; log out and back in to restart its background helper." },
};
export function MacosPermissionsPanel({ language }: { language: Language }) {
  const [permissions, setPermissions] = useState<Permissions | null>(null);
  const [error, setError] = useState("");
  const text = copy[language];
  const refresh = () => invoke<Permissions | null>("macos_permissions").then(setPermissions).catch((e) => setError(String(e)));
  useEffect(() => { void refresh(); }, []);
  if (!permissions) return null;
  async function open(permission: string) {
    setError("");
    try { await invoke("open_macos_permission_settings", { permission }); }
    catch (e) { setError(String(e)); }
  }
  return <div style={{ marginTop: 24 }}>
    <h2>{text.title}</h2><p>{text.hint}</p>
    <Space direction="vertical" style={{ marginTop: 12 }}>
      <Space><span>{text.screen}</span><Tag color={permissions.screenRecording ? "green" : "orange"}>{permissions.screenRecording ? text.allowed : text.missing}</Tag><Button onClick={() => void open("screen")}>{text.open}</Button></Space>
      <Space><span>{text.input}</span><Tag color={permissions.accessibility ? "green" : "orange"}>{permissions.accessibility ? text.allowed : text.missing}</Tag><Button onClick={() => void open("accessibility")}>{text.open}</Button></Space>
      <Button onClick={() => void refresh()}>{text.refresh}</Button>
    </Space>
    {error && <p role="alert">{error}</p>}
  </div>;
}
