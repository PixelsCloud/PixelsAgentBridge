import { userFacingError } from "./userFacingError";
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button, Progress } from "antd";
import { ArrowDownToLine, CheckCircle2 } from "lucide-react";
import type { Language } from "./i18n";

export type UpdateInfo = {
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
  const [installing, setInstalling] = useState(false);
  const busy = useRef(false);
  const [downloaded, setDownloaded] = useState(false);
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState("");
  const [installationMode, setInstallationMode] = useState("native");

  useEffect(() => {
    void invoke<string>("update_installation_mode").then(setInstallationMode).catch(() => {});
    void invoke<UpdateInfo | null>("update_status").then(setAvailable).catch(() => {});
    let closed = false;
    let offAvailable: (() => void) | undefined;
    let offProgress: (() => void) | undefined;
    void listen<UpdateInfo | null>("update-status-changed", event => {
      if (!closed) { setAvailable(event.payload); setDownloaded(false); }
    }).then(off => { if (closed) off(); else offAvailable = off; });
    void listen<DownloadProgress>("update-download-progress", event => {
      if (!closed) setProgress(Math.round(event.payload.received * 100 / event.payload.total));
    }).then(off => { if (closed) off(); else offProgress = off; });
    return () => { closed = true; offAvailable?.(); offProgress?.(); };
  }, []);

  async function check() {
    if (busy.current) return;
    setError(""); setChecking(true);
    try {
      const next = await invoke<UpdateInfo | null>("check_for_update");
      setAvailable(next); setDownloaded(false); setProgress(0);
    } catch (reason) { setError(String(reason)); }
    finally { setChecking(false); }
  }

  async function downloadAndInstall() {
    if (busy.current || checking) return;
    busy.current = true;
    setError(""); setDownloading(true);
    try {
      // Native download returns only after verification, including cached files.
      await invoke("download_update");
      setProgress(100); setDownloaded(true); setDownloading(false); setInstalling(true);
      await invoke("install_update");
      // A successful native launch exits the app. Keep actions locked until then.
    } catch (reason) {
      setError(String(reason)); setInstalling(false); busy.current = false;
    } finally { setDownloading(false); }
  }

  if (installationMode === "package") return <div className="settings-update">
    <div className="settings-update-heading"><div>
      <h3>{t("软件更新", "Updates")}</h3>
      <p>{t("安装新版 Linux 桌面安装包即可升级，账号、设备和设置会保留。", "Install a newer Linux desktop package to update. Your account, devices, and settings are preserved.")}</p>
    </div></div>
  </div>;

  return <div className="settings-update">
    <div className="settings-update-heading">
      <div>
        <h3>{t("在线更新", "Updates")}</h3>
        <p>{t("检查并安装新版本", "Check for and install new releases")}</p>
      </div>
      <Button loading={checking} disabled={downloading || installing} onClick={() => void check()}>{t("检查更新", "Check for updates")}</Button>
    </div>
    <div className="settings-update-status">
      {available ? <>
        <div className="settings-update-release">
          <ArrowDownToLine size={18} strokeWidth={1.8} aria-hidden="true" />
          <strong>{t("发现新版本", "New release")} V{available.version}</strong>
          <span>{(available.size / 1024 / 1024).toFixed(1)} MB</span>
        </div>
        <p className="settings-update-notes">{en ? available.notes_en : available.notes_zh}</p>
        <p>{t("下载并校验完成后将自动启动安装，请先保存正在进行的工作。", "Installation starts automatically after download and verification. Save your work first.")}</p>
        {downloading && <Progress percent={progress} />}
        <div className="settings-update-actions">
          <Button type="primary" loading={downloading || installing} disabled={checking}
            onClick={() => void downloadAndInstall()}>
            {installing ? t("正在启动安装", "Starting installation")
              : downloaded ? t("重试安装", "Retry installation")
              : t("下载并安装", "Download and install")}
          </Button>
        </div>
      </> : <div className="settings-update-current"><CheckCircle2 size={18} strokeWidth={1.8} aria-hidden="true" /><span>{t("没有待安装的更新。", "No update is available.")}</span></div>}
    </div>
    {error && <p className="settings-error" role="alert">{userFacingError(error, language, "update")}</p>}
  </div>;
}
