import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "antd";
import { ArrowDownToLine } from "lucide-react";
import type { Language } from "./i18n";
import type { UpdateInfo } from "./UpdatePanel";

export function UpdateNotice({ language, onOpen }: { language: Language; onOpen: () => void }) {
  const [available, setAvailable] = useState<UpdateInfo | null>(null);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void listen<UpdateInfo | null>("update-status-changed", event => {
      if (active) setAvailable(event.payload);
    }).then(off => {
      if (!active) off(); else {
        unlisten = off;
        void invoke<UpdateInfo | null>("update_status")
          .then(info => { if (active) setAvailable(info); })
          .catch(() => {});
      }
    }).catch(() => {});
    return () => { active = false; unlisten?.(); };
  }, []);

  if (!available) return null;

  const copy = language === "en"
    ? { title: "Update available", detail: "A new release is ready to download.", action: "View update" }
    : language === "zh-TW"
      ? { title: "發現新版本", detail: "新版本已可下載。", action: "查看更新" }
      : { title: "发现新版本", detail: "新版本已可下载。", action: "查看更新" };

  return <div className="update-notice" role="status" aria-live="polite">
    <span className="update-notice-icon"><ArrowDownToLine size={19} strokeWidth={2} /></span>
    <div className="update-notice-copy">
      <strong>{copy.title} v{available.version}</strong>
      <span>{copy.detail}</span>
    </div>
    <Button type="primary" onClick={onOpen}>{copy.action}</Button>
  </div>;
}
