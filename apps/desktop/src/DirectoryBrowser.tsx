import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { File, Folder, Link2 } from "lucide-react";
import { messages, type Language } from "./i18n";

type DirectoryEntry = {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  size: number | null;
};

type DirectoryPage = {
  requestId: string;
  path: string;
  entries: DirectoryEntry[];
  nextAfter: string | null;
};

export function DirectoryBrowser({ code, osFamily, connected, language, onAuditChange }: {
  code: string;
  osFamily: string;
  connected: boolean;
  language: Language;
  onAuditChange: () => void;
}) {
  const t = messages[language];
  const windows = osFamily.toLowerCase().includes("windows");
  const [path, setPath] = useState(windows ? "C:\\" : "/");
  const [page, setPage] = useState<DirectoryPage | null>(null);
  const [after, setAfter] = useState<string | null>(null);
  const [back, setBack] = useState<(string | null)[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    setPath(windows ? "C:\\" : "/");
    setPage(null);
    setAfter(null);
    setBack([]);
    setError("");
  }, [code, windows]);

  async function browse(target: string, cursor: string | null, direction: "first" | "next" | "back") {
    if (!target.trim() || loading) return;
    setLoading(true);
    setError("");
    try {
      const result = await invoke<DirectoryPage>("operator_list_directory", {
        code,
        path: target,
        after: cursor,
      });
      setPage(result);
      setPath(target);
      setAfter(cursor);
      if (direction === "first") setBack([]);
      if (direction === "next") setBack((current) => [...current, after]);
      if (direction === "back") setBack((current) => current.slice(0, -1));
      onAuditChange();
    } catch (cause) {
      setError(String(cause));
      onAuditChange();
    } finally {
      setLoading(false);
    }
  }

  function openChild(name: string) {
    const separator = windows ? "\\" : "/";
    void browse(`${path.replace(/[\\/]+$/, "")}${separator}${name}`, null, "first");
  }

  return <div className="directory-browser">
    <label className="field-label" htmlFor="remote-directory-path">{t.directoryPath}</label>
    <div className="directory-path-row">
      <input id="remote-directory-path" value={path} onChange={(event) => {
        setPath(event.target.value);
        setPage(null);
        setAfter(null);
        setBack([]);
      }} />
      <button className="quiet-button" disabled={!connected || loading || !path.trim()} onClick={() => void browse(path, null, "first")}>{loading ? t.loadingHistory : t.directoryBrowse}</button>
    </div>
    {error && <div className="inline-error" role="alert">{error}</div>}
    {page && <>
      <div className="directory-entries">
        {page.entries.length === 0 ? <small>{t.directoryEmpty}</small> : page.entries.map((entry) => (
          <button className="directory-entry" key={entry.name} disabled={entry.kind !== "directory" || loading} onClick={() => openChild(entry.name)}>
            <span>{entry.kind === "directory" ? <Folder size={17} /> : entry.kind === "file" ? <File size={17} /> : <Link2 size={17} />}</span>
            <strong title={entry.name}>{entry.name}</strong>
            <small>{entry.kind === "directory" ? t.directoryFolder : entry.kind === "symlink" ? t.directoryLink : entry.size == null ? t.directoryOther : `${entry.size.toLocaleString()} B`}</small>
          </button>
        ))}
      </div>
      <div className="directory-pages">
        <button className="quiet-button" disabled={back.length === 0 || loading} onClick={() => void browse(path, back[back.length - 1], "back")}>{t.directoryPrevious}</button>
        <button className="quiet-button" disabled={!page.nextAfter || loading} onClick={() => void browse(path, page.nextAfter, "next")}>{t.directoryNext}</button>
      </div>
    </>}
  </div>;
}
