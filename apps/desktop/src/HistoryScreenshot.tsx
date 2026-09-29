import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X } from "lucide-react";
import { messages, type Language } from "./i18n";

export function HistoryScreenshot({ id, language }: { id: string; language: Language }) {
  const t = messages[language];
  const [image, setImage] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [expanded, setExpanded] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setImage(null);
    setError("");
    setExpanded(false);
    void invoke<string | null>("operator_screenshot_from_history", { id })
      .then((result) => {
        if (active) setImage(result);
      })
      .catch((cause) => {
        if (active) setError(String(cause));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => { active = false; };
  }, [id]);

  useEffect(() => {
    if (!expanded) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setExpanded(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [expanded]);

  if (loading) return <p className="history-screenshot-note">{t.loadingHistory}</p>;
  if (error) return <p className="history-screenshot-note">{error}</p>;
  if (!image) return <p className="history-screenshot-note">{t.screenshotUnavailable}</p>;

  return <>
    <button className="history-screenshot-preview" onClick={() => setExpanded(true)} title={t.expandScreenshot} aria-label={t.expandScreenshot}>
      <img src={image} alt={t.screenshot} />
    </button>
    {expanded && <div className="screenshot-lightbox" role="dialog" aria-modal="true" aria-label={t.screenshot} onClick={() => setExpanded(false)}>
      <button className="screenshot-close" onClick={() => setExpanded(false)} aria-label={t.closeScreenshot}>{t.closeScreenshot} <X size={16} /></button>
      <img src={image} alt={t.screenshot} onClick={(event) => event.stopPropagation()} />
    </div>}
  </>;
}
