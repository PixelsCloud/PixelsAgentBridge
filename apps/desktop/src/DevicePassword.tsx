import { useEffect, useRef, useState } from "react";
import { App } from "antd";
import { Check, Copy, Eye, EyeOff, RefreshCw } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { messages, type Language } from "./i18n";
import type { DeviceStatus } from "./localDeviceStatus";

export function DevicePassword({ language, device, onRefreshed, onError }: {
  language: Language;
  device: DeviceStatus;
  onRefreshed: (status: DeviceStatus) => void;
  onError: (error: string) => void;
}) {
  const t = messages[language];
  const { message } = App.useApp();
  const [visible, setVisible] = useState(true);
  const [copied, setCopied] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const refreshingRef = useRef(false);

  useEffect(() => { setCopied(false); }, [device.temporary_password]);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1800);
    return () => window.clearTimeout(timer);
  }, [copied]);

  async function refreshPassword() {
    if (refreshingRef.current) return;
    refreshingRef.current = true;
    setRefreshing(true);
    setCopied(false);
    onError("");
    try {
      onRefreshed(await invoke<DeviceStatus>("rotate_device_password"));
      void message.success(t.passwordRefreshed);
    } catch {
      onError(t.passwordRefreshFailed);
    } finally {
      refreshingRef.current = false;
      setRefreshing(false);
    }
  }

  async function copyPassword() {
    try {
      await navigator.clipboard.writeText(device.temporary_password);
      setCopied(true);
      onError("");
    } catch { onError(t.copyFailed); }
  }

  return <div className="home-credential">
    <span className="code-label">{t.temporaryPassword}</span>
    <div className="home-credential-value home-password-value" aria-busy={refreshing}>
      <strong className={visible ? undefined : "masked-password"}>{visible ? device.temporary_password : (
        <span className="password-mask" role="img" aria-label={`${t.temporaryPassword} (${t.hide})`}>
          {Array.from({ length: 8 }, (_, index) => <span key={index} aria-hidden="true" />)}
        </span>
      )}</strong>
      <button className="credential-icon-button" aria-label={visible ? t.hide : t.show} title={visible ? t.hide : t.show} onClick={() => setVisible(!visible)}>
        {visible ? <EyeOff size={17} /> : <Eye size={17} />}
      </button>
      <button className="credential-icon-button" disabled={refreshing} aria-label={`${t.copy} ${t.temporaryPassword}`} title={copied ? t.copied : t.copy} onClick={() => void copyPassword()}>
        {copied ? <Check size={17} /> : <Copy size={17} />}
      </button>
      <button className="credential-icon-button" disabled={refreshing || !device.executor_running} aria-label={t.refreshPassword} title={t.refreshPasswordHint} onClick={() => void refreshPassword()}>
        <RefreshCw size={17} className={refreshing ? "password-refreshing" : undefined} />
      </button>
    </div>
  </div>;
}
