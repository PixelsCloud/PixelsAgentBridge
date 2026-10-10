import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button, Modal, Progress } from "antd";
import type { Language } from "./i18n";

type UpdateInfo = {
  version: string;
  size: number;
  notes_zh: string;
  notes_en: string;
};
type DownloadProgress = { received: number; total: number };

export function UpdatePanel({ language }: { language: Language }) {
  const en = language === "en";
  const t = (zh: string, english: string) => en ? english : zh;
  const [available, setAvailable] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [downloaded, setDownloaded] = useState(false);
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState("");

  useEffect(() => {
    void invoke<UpdateInfo | null>("update_status").then(setAvailable).catch(() => {});
    let closed = false;
    let offAvailable: (() => void) | undefined;
    let offProgress: (() => void) | undefined;
    void listen<UpdateInfo>("update-available", event => {
      if (!closed) { setAvailable(event.payload); setDownloaded(false); }
    }).then(off => { if (closed) off(); else offAvailable = off; });
    void listen<DownloadProgress>("update-download-progress", event => {
      if (!closed) setProgress(Math.round(event.payload.received * 100 / event.payload.total));
    }).then(off => { if (closed) off(); else offProgress = off; });
    return () => { closed = true; offAvailable?.(); offProgress?.(); };
  }, []);

  async function check() {
    setError(""); setChecking(true);
    try {
      const next = await invoke<UpdateInfo | null>("check_for_update");
      setAvailable(next); setDownloaded(false); setProgress(0);
    } catch (reason) { setError(String(reason)); }
    finally { setChecking(false); }
  }

  async function download() {
    setError(""); setDownloading(true);
    try { await invoke("download_update"); setProgress(100); setDownloaded(true); }
    catch (reason) { setError(String(reason)); }
    finally { setDownloading(false); }
  }

  function install() {
    Modal.confirm({
      title: t("安装更新？", "Install update?"),
      content: t("安装期间，当前远程控制、MCP 连接和任务可能中断。请先保存正在进行的工作。", "Remote control, MCP connections, and tasks may be interrupted. Save your work first."),
      okText: t("安装", "Install"), cancelText: t("取消", "Cancel"),
      onOk: async () => { try { await invoke("install_update"); } catch (reason) { setError(String(reason)); } },
    });
  }

  return <div className="settings-update">
    <h3>{t("在线更新", "Updates")}</h3>
    <p>{t("当前安装包版本", "Installed release")}：v{__PAB_RELEASE_VERSION__}</p>
    {available ? <>
      <p>{t("发现新版本", "New release")}：v{available.version} · {(available.size / 1024 / 1024).toFixed(1)} MB</p>
      <p>{en ? available.notes_en : available.notes_zh}</p>
      {downloading && <Progress percent={progress} />}
      {downloaded ? <Button type="primary" onClick={install}>{t("安装更新", "Install update")}</Button>
        : <Button type="primary" loading={downloading} onClick={() => void download()}>{t("下载更新", "Download update")}</Button>}
    </> : <p>{t("没有待安装的更新。", "No update is available.")}</p>}
    <Button loading={checking} onClick={() => void check()}>{t("检查更新", "Check for updates")}</Button>
    {error && <p className="settings-error" role="alert">{error}</p>}
  </div>;
}
