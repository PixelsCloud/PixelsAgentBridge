import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowDownToLine, ChevronRight } from "lucide-react";
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
    ? { title: "Update", action: "View update" }
    : language === "zh-TW"
      ? { title: "新版本", action: "查看更新" }
      : { title: "新版本", action: "查看更新" };

  const label = `${copy.title} V${available.version}`;
  return <div className="sidebar-update" role="status" aria-live="polite">
    <button type="button" onClick={onOpen} aria-label={`${label}，${copy.action}`} title={`${label} · ${copy.action}`}>
      <ArrowDownToLine size={15} strokeWidth={2} aria-hidden="true" />
      <span>{label}</span>
      <ChevronRight size={14} strokeWidth={1.8} aria-hidden="true" />
    </button>
  </div>;
}
