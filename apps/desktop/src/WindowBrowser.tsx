import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, AppWindow } from "lucide-react";
import { messages, type Language } from "./i18n";

type WindowEntry = { title: string; process_id: number };
type WindowList = { requestId: string; entries: WindowEntry[] };

export function WindowBrowser({ code, connected, language, onAuditChange }: {
  code: string;
  connected: boolean;
  language: Language;
  onAuditChange: () => void;
}) {
  const t = messages[language];
  const [entries, setEntries] = useState<WindowEntry[]>([]);
  const [page, setPage] = useState(0);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    setEntries([]);
    setPage(0);
    setError("");
  }, [code]);

  async function refresh() {
    if (!connected || loading) return;
    setLoading(true);
    setError("");
    try {
      const result = await invoke<WindowList>("operator_list_windows", { code });
      setEntries(result.entries);
      setPage(0);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
      onAuditChange();
    }
  }

  const visible = entries.slice(page * 8, (page + 1) * 8);
  return <div className="directory-browser">
    <div className="list-heading"><h3>{t.windowList}</h3><span>{entries.length}</span></div>
    <p className="form-hint">{t.windowListHint}</p>
    <button className="primary-button" disabled={!connected || loading} onClick={() => void refresh()}>
      {loading ? t.loadingHistory : t.refreshWindows}<RefreshCw size={16} />
    </button>
    {error && <div className="inline-error" role="alert">{error}</div>}
    {entries.length === 0 && !error && !loading && <div className="empty-list">{t.noWindows}</div>}
    {visible.length > 0 && <>
      <div className="directory-entries">
        {visible.map((entry, index) => <div className="directory-entry" key={`${entry.process_id}-${index}`}>
          <span><AppWindow size={17} /></span><strong title={entry.title}>{entry.title}</strong><small>PID {entry.process_id}</small>
        </div>)}
      </div>
      <div className="directory-pages">
        <button className="quiet-button" disabled={page === 0} onClick={() => setPage(page - 1)}>{t.directoryPrevious}</button>
        <button className="quiet-button" disabled={(page + 1) * 8 >= entries.length} onClick={() => setPage(page + 1)}>{t.directoryNext}</button>
      </div>
    </>}
  </div>;
}
