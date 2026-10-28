import { useEffect, useRef, useState, type ReactNode } from "react";
import { ArrowLeft, ArrowRight, ChevronDown, HardDrive, Home, Download, Monitor, FileText, Image, Music, Film, ArrowUp, Copy, File, Folder, FolderOpen, FolderPlus, Info, Link2, Pencil, RefreshCw, Search, Trash2, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { FileBreadcrumbs } from "./FileBreadcrumbs";
import type { ExecutionSelection } from "./executionQueries";
import { initialLanguage } from "./i18n";
import { directoryChangeKey, type DirectoryTransfer } from "./fileDirectoryRefresh";
import { defaultFilter, displayFileText, fileExecutionChanged, FileRequestError, fileRequest, fileSize, nativePathKey, observeFileRequest, pathLabel, utf8Path, type FileEntry, type FileFilter, type FileOperation, type FileLocations, type FilePage, type NativePath } from "./fileManagerApi";

export type PaneSelection = { endpoint: string; execution: ExecutionSelection; path: NativePath | null; entries: FileEntry[]; pathStyle: "windows" | "posix" };
const cleanPathLabel = pathLabel;
const shortcuts: Record<string, { label: string; icon: typeof Folder }> = {
  Desktop: { label: initialLanguage() === "en" ? "Desktop" : "桌面", icon: Monitor },
  Downloads: { label: initialLanguage() === "en" ? "Downloads" : "下载", icon: Download },
  Documents: { label: initialLanguage() === "en" ? "Documents" : "文档", icon: FileText },
  Pictures: { label: initialLanguage() === "en" ? "Pictures" : "图片", icon: Image },
  Music: { label: initialLanguage() === "en" ? "Music" : "音乐", icon: Music },
  Videos: { label: initialLanguage() === "en" ? "Videos" : "视频", icon: Film },
};
const service: ExecutionSelection = { mode: "service" };
const en = initialLanguage() === "en";
const text = en ? { local: "This computer", parent: "Parent folder", refresh: "Refresh", mkdir: "New folder", rename: "Rename", remove: "Delete", info: "Properties", hidden: "Hidden files", search: "Search this folder", name: "Name", size: "Size", modified: "Modified", reading: "Reading folder…", empty: "No matching items", cancel: "Cancel", confirm: "Confirm", permanent: "Permanently delete the selected items? This cannot be undone.", selected: "selected", previous: "Previous page", next: "Next page", native: "Native path (display only)", error: "Unable to read folder", retry: "Try again" } : { local: "本机", parent: "上级目录", refresh: "刷新", mkdir: "新建文件夹", rename: "重命名", remove: "删除", info: "属性", hidden: "隐藏文件", search: "搜索当前目录", name: "名称", size: "大小", modified: "修改时间", reading: "正在读取目录…", empty: "没有匹配的项目", cancel: "取消", confirm: "确定", permanent: "永久删除选中的项目？此操作无法撤销。", selected: "已选择", previous: "上一页", next: "下一页", native: "原生路径（仅供显示）", error: "目录读取失败", retry: "重试" };
type Props = { endpoint: string; execution?: ExecutionSelection; executionIdentityKey?: string; onExecutionUnavailable?: () => void; title: string; forceRelay?: boolean; disabled?: boolean; headingDetail?: ReactNode; transferAction?: ReactNode; batchUpdates?: readonly DirectoryTransfer[]; onSelection: (selection: PaneSelection) => void; onDragSelection?: (selection: PaneSelection | null, token: string | null) => void };

export function FilePane({ endpoint, execution = service, executionIdentityKey, onExecutionUnavailable, title, forceRelay = false, disabled = false, headingDetail, transferAction, batchUpdates = [], onSelection, onDragSelection }: Props) {
  const executionKey = JSON.stringify(execution);
  const identityKey = executionIdentityKey ?? executionKey;
  const initializedScope = useRef("");
  const request = (operation: FileOperation, signal?: AbortSignal) => fileRequest(endpoint, execution, operation, signal, forceRelay);
  const readFailed = (cause: unknown) => { setError(String(cause)); if (execution.mode === "user" && fileExecutionChanged(cause)) onExecutionUnavailable?.(); };
  const [locations, setLocations] = useState<FileLocations | null>(null);
  const [path, setPath] = useState<NativePath | null>(null);
  const [draft, setDraft] = useState("");
  const [editingPath, setEditingPath] = useState(false);
  const [crumbs, setCrumbs] = useState<{ label: string; path: NativePath }[]>([]);
  const [snapshot, setSnapshot] = useState<string | null>(null);
  const [page, setPage] = useState<FilePage | null>(null);
  const [pageScope, setPageScope] = useState("");
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<FileFilter>(defaultFilter);
  const [navigation, setNavigation] = useState({ key: "", cursors: [null] as (string | null)[], index: 0 });
  const [selected, setSelected] = useState<Map<string, FileEntry>>(new Map());
  const [reading, setReading] = useState(false), [scanning, setScanning] = useState(false);
  const [error, setError] = useState(""), [reload, setReload] = useState(0);
  const [dialog, setDialog] = useState<"mkdir" | "rename" | "delete" | "properties" | null>(null);
  const [name, setName] = useState(""), [mutating, setMutating] = useState(false), [details, setDetails] = useState<FileEntry | null>(null);
  const [unknown, setUnknown] = useState<string | null>(null), [deleted, setDeleted] = useState<string[]>([]);
  const [unknownEntry, setUnknownEntry] = useState<FileEntry | null>(null), [unknownDialog, setUnknownDialog] = useState<typeof dialog>(null);
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number } | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [refreshPending, setRefreshPending] = useState(false);
  const observedChanges = useRef({ scope: "", key: "" });
  const lastRefresh = useRef(0);
  const lastPageView = useRef({ scope: "", filter: "", cursor: null as string | null });
  const history = useRef<{ paths: NativePath[]; index: number }>({ paths: [], index: -1 });
  const anchor = useRef<number | null>(null), list = useRef<HTMLDivElement>(null);
  const locationMenu = useRef<HTMLDetailsElement>(null);
  const dialogForm = useRef<HTMLFormElement>(null), menu = useRef<HTMLDivElement>(null), mutationGate = useRef(false), mutationBusy = useRef(false);
  mutationBusy.current = mutating;
  const onSelectionRef = useRef(onSelection); onSelectionRef.current = onSelection;
  const pathKey = path ? nativePathKey(path) : "";
  const scope = JSON.stringify([endpoint, identityKey, forceRelay, pathKey]);
  const filterKey = JSON.stringify(filter);
  const viewKey = `${snapshot}:${filterKey}`;
  const cursor = navigation.key === viewKey ? navigation.cursors[navigation.index] : null;
  const pageIndex = navigation.key === viewKey ? navigation.index : 0;
  const validPage = page && pageScope === scope;
  const busy = reading || scanning || disabled || !!validPage && page.snapshot_id !== snapshot;
  const changeKey = directoryChangeKey(batchUpdates, endpoint, path, locations?.path_style || "posix");
  useEffect(() => {
    if (observedChanges.current.scope !== scope) {
      observedChanges.current = { scope, key: changeKey };
      setRefreshPending(false); return;
    }
    if (observedChanges.current.key !== changeKey) {
      observedChanges.current.key = changeKey;
      setRefreshPending(true);
    }
  }, [scope, changeKey]);
  useEffect(() => {
    if (!refreshPending || !path || disabled || reading || scanning || dialog || contextMenu || mutating || unknown) return;
    // Coalesce item completions, never rescan for byte/revision-only progress.
    // A dirty view waits for its active scan/dialog; it is not forgotten.
    const timer = setTimeout(() => {
      lastRefresh.current = performance.now();
      setRefreshPending(false); setReload(value => value + 1);
    }, Math.max(250, 2000 - (performance.now() - lastRefresh.current)));
    return () => clearTimeout(timer);
  }, [refreshPending, scope, disabled, reading, scanning, dialog, contextMenu, mutating, unknown]);
  useEffect(() => {
    const close = (event: PointerEvent) => { if (!locationMenu.current?.contains(event.target as Node)) locationMenu.current?.removeAttribute("open"); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape" && locationMenu.current?.open) { locationMenu.current.removeAttribute("open"); locationMenu.current.querySelector("summary")?.focus(); } };
    document.addEventListener("pointerdown", close); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", escape); };
  }, []);
  useEffect(() => {
    if (!dialog) return;
    const previous = document.activeElement as HTMLElement | null;
    const elements = () => [...(dialogForm.current?.querySelectorAll<HTMLElement>("button:enabled,input:enabled,[tabindex='0']") || [])];
    (dialogForm.current?.querySelector<HTMLElement>("input") || elements()[0])?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !mutationBusy.current) { event.preventDefault(); setDialog(null); }
      if (event.key === "Tab") { const targets = elements(), index = targets.indexOf(document.activeElement as HTMLElement); if (event.shiftKey && index <= 0) { event.preventDefault(); targets[targets.length - 1]?.focus(); } else if (!event.shiftKey && index === targets.length - 1) { event.preventDefault(); targets[0]?.focus(); } }
    };
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("keydown", key); if (previous?.isConnected) previous.focus(); };
  }, [dialog]);
  useEffect(() => {
    if (!contextMenu) return;
    const previous = document.activeElement as HTMLElement | null;
    const targets = () => [...(menu.current?.querySelectorAll<HTMLButtonElement>("button:enabled") || [])];
    targets()[0]?.focus();
    const key = (event: KeyboardEvent) => {
      if (["Escape", "Tab"].includes(event.key)) { event.preventDefault(); setContextMenu(null); }
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        event.preventDefault(); const items = targets(), index = items.indexOf(document.activeElement as HTMLButtonElement);
        items[event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : (index + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
      }
    };
    window.addEventListener("keydown", key); return () => { window.removeEventListener("keydown", key); if (previous?.isConnected) previous.focus(); };
  }, [contextMenu]);

  useEffect(() => { const timer = setTimeout(() => setFilter(value => ({ ...value, query: search })), 300); return () => clearTimeout(timer); }, [search]);
  useEffect(() => {
    let alive = true;
    const boundary = JSON.stringify([endpoint, identityKey, forceRelay]);
    if (initializedScope.current !== boundary) {
      initializedScope.current = boundary;
      setLocations(null); setPath(null); setPage(null); setSnapshot(null); setSelected(new Map()); setError("");
      history.current = { paths: [], index: -1 };
    }
    if (disabled) return;
    void request({ action: "locations" }).then(reply => {
      if (!alive || reply.result.result !== "locations") return;
      const roots = reply.result.locations;
      setLocations(roots);
      const initial = roots.home || roots.roots[0]?.path;
      // A renewed connection reference for the same frozen native identity
      // retains navigation and unresolved mutation state. A new identity has
      // its own boundary above and never inherits this directory.
      if (initial) setPath(current => { if (current) return current; history.current = { paths: [initial], index: 0 }; return initial; });
    }).catch(cause => { if (alive) readFailed(cause); });
    return () => { alive = false; };
  }, [endpoint, identityKey, executionKey, forceRelay, disabled]);

  useEffect(() => {
    setDraft(path ? cleanPathLabel(path) : ""); setEditingPath(false);
    if (!path || disabled) return;
    const controller = new AbortController(); let opened: string | null = null;
    setScanning(true); setReading(false); setError(""); setSnapshot(null);
    void request({ action: "open_directory", path }, controller.signal).then(reply => {
      if (reply.result.result !== "opened") throw new Error("Invalid directory response");
      opened = reply.result.snapshot_id;
      if (!controller.signal.aborted) { setSnapshot(opened); setScanning(false); }
    }).catch(cause => { if (!controller.signal.aborted) { readFailed(cause); setScanning(false); } });
    return () => {
      controller.abort();
      if (opened) void request({ action: "close_directory", snapshot_id: opened }).catch(() => undefined);
    };
  }, [pathKey, endpoint, executionKey, forceRelay, reload, disabled]);

  useEffect(() => {
    if (!snapshot || disabled) return;
    let alive = true;
    setReading(true); setError("");
    void request({ action: "read_directory", snapshot_id: snapshot, after: cursor, limit: 256, filter }).then(reply => {
      if (!alive || reply.result.result !== "directory") return;
      const sameView = lastPageView.current.scope === scope && lastPageView.current.filter === filterKey && lastPageView.current.cursor === cursor;
      lastPageView.current = { scope, filter: filterKey, cursor };
      setPage(reply.result.page); setPageScope(scope); setReading(false); anchor.current = null;
      // Refreshing a snapshot must not clear selection or jump an unchanged
      // first-page viewport. A new opaque query starts on its first page.
      if (!sameView) { setScrollTop(0); if (list.current) list.current.scrollTop = 0; }
      setSelected(current => {
        let next = current;
        for (const entry of reply.result.result === "directory" ? reply.result.page.entries : []) {
          const key = nativePathKey(entry.path);
          if (current.has(key)) { if (next === current) next = new Map(current); next.set(key, entry); }
        }
        return next;
      });
    }).catch(cause => { if (alive) { readFailed(cause); setReading(false); } });
    return () => { alive = false; };
  }, [snapshot, viewKey, cursor, endpoint, executionKey, forceRelay, disabled]);

  useEffect(() => {
    onSelectionRef.current({ endpoint, execution, path: validPage && !busy ? path : null, entries: [...selected.values()], pathStyle: locations?.path_style || "posix" });
  }, [selected, pathKey, endpoint, executionKey, forceRelay, locations, busy, snapshot, page]);

  useEffect(() => {
    let alive = true; setCrumbs([]);
    if (!path || !locations) return;
    void (async () => {
      const items: { label: string; path: NativePath }[] = [];
      let current: NativePath | null = path;
      for (let depth = 0; current && depth < 128; depth++) {
        const label = cleanPathLabel(current);
        const parent: NativePath | null = await invoke<NativePath | null>("file_manager_path", { path: current, pathStyle: locations.path_style, name: null });
        const root = !parent || nativePathKey(parent) === nativePathKey(current);
        items.unshift({ label: root ? label : label.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || label, path: current });
        if (root) break;
        current = parent;
      }
      if (alive) setCrumbs(items);
    })().catch(() => { if (alive) setCrumbs([{ label: cleanPathLabel(path), path }]); });
    return () => { alive = false; };
  }, [pathKey, locations?.path_style]);
  function navigate(next: NativePath, record = true) {
    if (disabled) return;
    if (record) { const current = history.current; const paths = [...current.paths.slice(0, current.index + 1), next].slice(-64); history.current = { paths, index: paths.length - 1 }; }
    setPath(next); setSelected(new Map());
  }
  function stepHistory(delta: number) {
    const next = history.current.index + delta;
    if (next >= 0 && next < history.current.paths.length) { history.current.index = next; navigate(history.current.paths[next], false); }
  }
  async function parent() {
    if (!path || !locations) return;
    try { const next = await invoke<NativePath | null>("file_manager_path", { path, pathStyle: locations.path_style, name: null }); if (next) navigate(next); }
    catch (cause) { setError(String(cause)); }
  }
  function select(entry: FileEntry, index: number, additive: boolean, range = false) {
    setSelected(current => {
      const next = additive ? new Map(current) : new Map<string, FileEntry>();
      if (range && anchor.current !== null && page) for (const row of page.entries.slice(Math.min(anchor.current, index), Math.max(anchor.current, index) + 1)) next.set(nativePathKey(row.path), row);
      else if (additive && next.has(nativePathKey(entry.path))) next.delete(nativePathKey(entry.path)); else next.set(nativePathKey(entry.path), entry);
      return next.size <= 4096 ? next : current;
    });
    if (!range) anchor.current = index;
  }
  function openDialog(kind: NonNullable<typeof dialog>) {
    if (unknown) { setContextMenu(null); setDialog(unknownDialog); return; }
    setContextMenu(null); setUnknown(null); setDeleted([]);
    const original=[...selected.values()][0];
    setName(kind === "rename" && original?.path.encoding === "utf8" && displayFileText(original.name) === original.name ? original.name : ""); setDetails(kind === "properties" ? [...selected.values()][0] || null : null); setDialog(kind); setError("");
    if (kind === "properties" && selected.size === 1) void request({ action: "inspect", path: [...selected.values()][0].path }).then(reply => { if (reply.result.result === "entry") setDetails(reply.result.entry); }).catch(cause => setError(String(cause)));
  }
  async function mutate() {
    if (mutationGate.current || unknown || !path) return;
    mutationGate.current = true;
    setMutating(true); setError("");
    let attempted: FileEntry | null = null;
    try {
      if (dialog === "mkdir") await request({ action: "create_directory", parent: path, name: utf8Path(name) });
      else if (dialog === "rename" && selected.size === 1) { attempted = [...selected.values()][0]; await request({ action: "rename", path: attempted.path, name: utf8Path(name) }); }
      else if (dialog === "delete") for (const entry of selected.values()) {
        attempted = entry;
        await request({ action: "delete", path: entry.path, recursive: entry.kind === "directory" });
        setDeleted(current => [...current, entry.name]);
        setSelected(current => { const next = new Map(current); next.delete(nativePathKey(entry.path)); return next; });
      }
      setDialog(null); setSelected(new Map()); setReload(value => value + 1);
    } catch (cause) { setError(`${attempted ? `${attempted.name}: ` : ""}${String(cause)}`); if (cause instanceof FileRequestError && cause.unconfirmed) { setUnknown(cause.requestId); setUnknownEntry(attempted); setUnknownDialog(dialog); } }
    finally { setMutating(false); mutationGate.current = false; }
  }
  async function inspectUnknown() {
    if (!unknown || mutationGate.current) return;
    mutationGate.current = true; setMutating(true);
    try {
      const frame = await observeFileRequest(unknown);
      if (frame.reply?.result.result === "applied") {
        setUnknown(null); setUnknownDialog(null);
        if (unknownDialog === "delete" && unknownEntry) {
          setDeleted(current => [...current, unknownEntry.name]);
          setSelected(current => { const next = new Map(current); next.delete(nativePathKey(unknownEntry.path)); return next; });
          if (selected.size <= 1) setDialog(null);
        } else { setDialog(null); setSelected(new Map()); }
        setUnknownEntry(null); setReload(value => value + 1); setError("");
      }
      else if (frame.reply?.result.result === "error" && frame.reply.result.error.code !== "unconfirmed") { setUnknown(null); setUnknownEntry(null); setUnknownDialog(null); setError(frame.reply.result.error.message); }
    } catch (cause) { setError(String(cause)); }
    finally { setMutating(false); mutationGate.current = false; }
  }
  async function copyPath() {
    const selectedPath = selected.size === 1 ? [...selected.values()][0].path : path;
    if (!selectedPath) return;
    if (selectedPath.encoding !== "utf8") { setError(en ? "This native path cannot be represented as clipboard text. Use its entry to transfer or manage the file." : "此原生路径无法表示为剪贴板文本，请直接选择条目进行传输或管理。"); setContextMenu(null); return; }
    try { await navigator.clipboard.writeText(selectedPath.value); setContextMenu(null); } catch (cause) { setError(String(cause)); }
  }
  async function revealLocal() {
    const target=selected.size===1?[...selected.values()][0].path:path;
    if (endpoint!=="local" || !target || busy) return;
    try {await invoke("file_manager_reveal_local",{path:target});setContextMenu(null);}
    catch (cause) {setError(String(cause));}
  }
  const rows = validPage ? page.entries : [];
  const first = Math.max(0, Math.floor(scrollTop / 38) - 4), visible = rows.slice(first, first + 32);
  return <section className="fm-pane" aria-label={title}>
    <header className="fm-pane-heading"><strong>{title}</strong>{headingDetail}</header>
    <div className="fm-pathbar">
      {endpoint !== "local" && transferAction}
      <button title={en ? "Back" : "后退"} disabled={history.current.index <= 0 || disabled} onClick={() => stepHistory(-1)}><ArrowLeft size={16} /></button>
      <button title={en ? "Forward" : "前进"} disabled={history.current.index >= history.current.paths.length - 1 || disabled} onClick={() => stepHistory(1)}><ArrowRight size={16} /></button>
      <button title={text.parent} disabled={!path || disabled} onClick={() => void parent()}><ArrowUp size={16} /></button>
      <div className="fm-path-control">
        {editingPath ? <form onSubmit={event => { event.preventDefault(); if (draft && draft !== (path && cleanPathLabel(path))) navigate(utf8Path(draft)); setEditingPath(false); }}><input autoFocus aria-label={`${title} ${en ? "path" : "路径"}`} value={draft} disabled={disabled} onChange={event => setDraft(event.target.value)} onBlur={() => setEditingPath(false)} onKeyDown={event => { if (event.key === "Escape") setEditingPath(false); }} spellCheck={false}/></form> : <FileBreadcrumbs items={crumbs} label={`${title} ${en ? "path" : "路径"}`} disabled={disabled} onNavigate={navigate} pathTitle={cleanPathLabel} moreLabel={en ? "Intermediate folders" : "中间目录"}/>}
        <button title={en ? "Edit path" : "编辑路径"} disabled={disabled || !path || path.encoding !== "utf8" || displayFileText(path.value) !== path.value} onClick={() => setEditingPath(true)}><Pencil size={14}/></button>
      </div>
      <details ref={locationMenu} className="fm-location-picker"><summary aria-label={en ? "Disks and folders" : "磁盘与常用目录"}><FolderOpen size={16}/><ChevronDown size={13}/></summary><div className="fm-location-menu">
        {locations?.home && <button disabled={disabled} onClick={event => { navigate(locations.home!); event.currentTarget.closest("details")?.removeAttribute("open"); }}><Home size={16}/>{en ? "Home" : "用户目录"}</button>}
        {locations?.roots.map(root => { const shortcut = shortcuts[root.name]; const Icon = shortcut?.icon || HardDrive; return <button key={nativePathKey(root.path)} disabled={disabled} title={cleanPathLabel(root.path)} onClick={event => { navigate(root.path); event.currentTarget.closest("details")?.removeAttribute("open"); }}><Icon size={16}/>{shortcut ? shortcut.label : cleanPathLabel(root.path)}</button>; })}
      </div></details>
      {endpoint === "local" && transferAction}
    </div>

    <div className="fm-tools">
      <button title={text.mkdir} disabled={!path || busy} onClick={() => openDialog("mkdir")}><FolderPlus size={16} /></button>
      <button title={text.rename} disabled={selected.size !== 1 || busy} onClick={() => openDialog("rename")}><Pencil size={16} /></button>
      <button title={text.remove} disabled={!selected.size || busy} onClick={() => openDialog("delete")}><Trash2 size={16} /></button>
      <button title={text.info} disabled={selected.size !== 1 || busy} onClick={() => openDialog("properties")}><Info size={16} /></button>
      <button title={en ? "Copy path" : "复制路径"} disabled={!path || selected.size > 1 || busy} onClick={() => void copyPath()}><Copy size={16} /></button>
      {endpoint === "local" && <button title={en ? "Show in file manager" : "在文件管理器中显示"} disabled={!path || selected.size > 1 || busy} onClick={() => void revealLocal()}><FolderOpen size={16} /></button>}
      <label className="fm-search"><Search size={15} /><input aria-label={`${title} ${text.search}`} placeholder={text.search} value={search} onChange={event => setSearch(event.target.value)} /></label>
      <label className="fm-hidden"><input type="checkbox" checked={filter.show_hidden} onChange={event => setFilter(value => ({ ...value, show_hidden: event.target.checked }))} />{text.hidden}</label>
      <button title={text.refresh} disabled={!path || disabled} onClick={() => setReload(value => value + 1)}><RefreshCw size={16} /></button>
    </div>
    <div className="fm-columns">{([ ["name", text.name], ["size", text.size], ["modified", text.modified] ] as const).map(([sort, label]) => <button key={sort} onClick={() => setFilter(value => ({ ...value, sort, descending: value.sort === sort ? !value.descending : false }))}>{label}{filter.sort === sort ? filter.descending ? " ↓" : " ↑" : ""}</button>)}</div>
    <div ref={list} className="fm-file-list" role="listbox" aria-label={`${title} ${text.name}`} aria-multiselectable tabIndex={0} aria-busy={busy} onScroll={event => setScrollTop(event.currentTarget.scrollTop)} onKeyDown={event => {
      if (busy) return;
      if ((event.key === "ContextMenu" || event.shiftKey && event.key === "F10") && selected.size) { event.preventDefault(); const bounds = list.current?.querySelector("[aria-selected='true']")?.getBoundingClientRect() || list.current!.getBoundingClientRect(); setContextMenu({ x: Math.max(0, Math.min(bounds.left + 30, window.innerWidth - 210)), y: Math.max(0, Math.min(bounds.top + 25, window.innerHeight - 220)) }); }
      if ((event.ctrlKey || event.metaKey) && event.key === "a") { event.preventDefault(); setSelected(new Map(rows.map(entry => [nativePathKey(entry.path), entry]))); }
      if (event.key === "Delete" && selected.size) { event.preventDefault(); openDialog("delete"); }
      if (event.key === "F2" && selected.size === 1) { event.preventDefault(); openDialog("rename"); }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "c") { event.preventDefault(); void copyPath(); }
      if (event.key === "Enter" && selected.size === 1) { const entry = [...selected.values()][0]; if (entry.kind === "directory") { event.preventDefault(); navigate(entry.path); } }
      if (event.key === "Backspace") { event.preventDefault(); void parent(); }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); const current = rows.findIndex(entry => selected.has(nativePathKey(entry.path))); const index = Math.max(0, Math.min(rows.length - 1, current + (event.key === "ArrowDown" ? 1 : -1))); const entry = rows[index]; if (entry) { select(entry, index, event.ctrlKey || event.metaKey, event.shiftKey); const element = list.current; if (element) { const top = index * 38; if (top < element.scrollTop) element.scrollTop = top; else if (top + 38 > element.scrollTop + element.clientHeight) element.scrollTop = top + 38 - element.clientHeight; } } }
      if (event.key === "F5") { event.preventDefault(); setReload(value => value + 1); }
    }}>
      <div style={{ height: rows.length * 38, position: "relative" }}>{visible.map((entry, offset) => <div role="option" draggable={!busy && !!onDragSelection} onDragStart={event => {
        if (busy || !path || !onDragSelection) { event.preventDefault(); return; }
        const entries=selected.has(nativePathKey(entry.path))?[...selected.values()]:[entry];
        const token=crypto.randomUUID();event.dataTransfer.effectAllowed="copy";event.dataTransfer.setData("application/x-pixels-file-selection",token);
        onDragSelection({endpoint,execution,path,entries,pathStyle:locations?.path_style || "posix"},token);
      }} onDragEnd={() => onDragSelection?.(null,null)} aria-selected={selected.has(nativePathKey(entry.path))} key={nativePathKey(entry.path)} className={`fm-file-row ${selected.has(nativePathKey(entry.path)) ? "selected" : ""} ${entry.hidden ? "hidden-file" : ""}`} style={{ top: (first + offset) * 38 }} onClick={event => { if (!busy) select(entry, first + offset, event.ctrlKey || event.metaKey, event.shiftKey); }} onContextMenu={event => { event.preventDefault(); if (busy) return; if (!selected.has(nativePathKey(entry.path))) setSelected(new Map([[nativePathKey(entry.path), entry]])); setContextMenu({ x: Math.min(event.clientX, window.innerWidth - 210), y: Math.min(event.clientY, window.innerHeight - 220) }); }} onDoubleClick={() => { if (entry.kind === "directory" && !busy) navigate(entry.path); }}>
        <span className="fm-file-name" title={entry.error || displayFileText(entry.name)}><input aria-label={`${en ? "Select" : "选择"} ${displayFileText(entry.name)}`} type="checkbox" checked={selected.has(nativePathKey(entry.path))} disabled={busy} onClick={event => event.stopPropagation()} onChange={() => select(entry, first + offset, true)} />{entry.kind === "directory" ? <Folder size={17} /> : entry.kind === "symlink" ? <Link2 size={17} /> : <File size={17} />}<span>{displayFileText(entry.name)}</span>{entry.error && <Info size={14} />}</span>
        <span>{entry.kind === "directory" ? "—" : fileSize(entry.size)}</span><time>{entry.modified_at_unix_ms === null ? "—" : new Date(entry.modified_at_unix_ms).toLocaleString(undefined, { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })}</time>
      </div>)}</div>
      {!rows.length && !busy && !error && <div className="fm-empty">{text.empty}</div>}
    </div>
    {error && <div className="fm-error" role="alert">{error}</div>}
    <footer className="fm-pane-status"><span role="status">{busy ? text.reading : `${page?.total_entries || "0"} ${en ? "items" : "项"} · ${selected.size} ${text.selected}`}</span><div><button title={text.previous} disabled={pageIndex === 0 || busy} onClick={() => setNavigation(value => ({ ...value, index: value.index - 1 }))}><ArrowLeft size={14} /></button><span>{pageIndex + 1}</span><button title={text.next} disabled={!page?.next_after || busy} onClick={() => setNavigation(value => { const cursors = value.key === viewKey ? value.cursors.slice(0, value.index + 1) : [null]; return { key: viewKey, cursors: [...cursors, page!.next_after], index: cursors.length }; })}><ArrowRight size={14} /></button></div></footer>
    {contextMenu && <div className="fm-menu-shade" onMouseDown={() => setContextMenu(null)}><div ref={menu} className="fm-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }} onMouseDown={event => event.stopPropagation()}>
      {selected.size === 1 && [...selected.values()][0].kind === "directory" && <button role="menuitem" onClick={() => { navigate([...selected.values()][0].path); setContextMenu(null); }}>{en ? "Open folder" : "打开文件夹"}</button>}
      <button role="menuitem" disabled={selected.size !== 1} onClick={() => void copyPath()}>{en ? "Copy path" : "复制路径"}</button>
      {endpoint === "local" && <button role="menuitem" disabled={selected.size !== 1} onClick={() => void revealLocal()}>{en ? "Show in file manager" : "在文件管理器中显示"}</button>}
      <button role="menuitem" disabled={selected.size !== 1} onClick={() => openDialog("rename")}>{text.rename}</button>
      <button role="menuitem" onClick={() => openDialog("delete")}>{text.remove}</button>
      <button role="menuitem" disabled={selected.size !== 1} onClick={() => openDialog("properties")}>{text.info}</button>
    </div></div>}
    {dialog && <div className="fm-modal-shade"><form ref={dialogForm} className="fm-dialog" role="dialog" aria-modal="true" aria-label={dialog === "mkdir" ? text.mkdir : dialog === "rename" ? text.rename : dialog === "delete" ? text.remove : text.info} onSubmit={event => { event.preventDefault(); if (dialog !== "properties") void mutate(); }}>
      <header><strong>{dialog === "mkdir" ? text.mkdir : dialog === "rename" ? text.rename : dialog === "delete" ? text.remove : text.info}</strong><button type="button" title={text.cancel} disabled={mutating} onClick={() => setDialog(null)}><X size={18} /></button></header>
      {dialog === "delete" ? <><p>{text.permanent}<br />{selected.size} {en ? "items" : "项"}</p><ul>{[...selected.values()].slice(0, 20).map(entry => <li key={nativePathKey(entry.path)}>{pathLabel(entry.path)}</li>)}</ul></> : dialog === "properties" ? <dl><dt>{text.name}</dt><dd>{details && displayFileText(details.name)}</dd><dt>{en ? "Path" : "路径"}</dt><dd>{details && pathLabel(details.path)}</dd><dt>{text.size}</dt><dd>{fileSize(details?.size ?? null)}</dd><dt>{text.modified}</dt><dd>{details?.modified_at_unix_ms == null ? "—" : new Date(details.modified_at_unix_ms).toLocaleString()}</dd><dt>{en ? "Permissions" : "权限"}</dt><dd>{details?.unix_mode != null ? details.unix_mode.toString(8) : details?.readonly ? en ? "Read only" : "只读" : "—"}</dd></dl> : <input aria-label={text.name} autoFocus value={name} onChange={event => setName(event.target.value)} maxLength={1024} required />}
      {!!deleted.length && <p>{en ? "Deleted" : "已删除"}: {deleted.map(displayFileText).join(", ")}</p>}
      {dialog === "rename" && [...selected.values()].some(entry=>entry.path.encoding!=="utf8" || displayFileText(entry.name)!==entry.name) && <p>{en ? "The current name contains characters that cannot be edited directly. Enter a new name." : "当前名称包含无法直接编辑的字符，请输入新名称。"}</p>}
      {unknown && <p>{en ? "This operation is unconfirmed. Inspect its original request before retrying." : "本次操作的结果尚未确认，请先核实原请求，避免重复执行。"}</p>}
      {unknownEntry && <p className="fm-drop-destination">{pathLabel(unknownEntry.path)}</p>}
      {error && <p role="alert" className="fm-error">{error}</p>}
      <footer>{unknown && <button type="button" disabled={mutating} onClick={() => void inspectUnknown()}>{en ? "Inspect original request" : "核实原请求"}</button>}<button type="button" disabled={mutating} onClick={() => setDialog(null)}>{dialog === "properties" ? en ? "Close" : "关闭" : text.cancel}</button>{dialog !== "properties" && <button className={dialog === "delete" ? "danger" : "primary"} type="submit" disabled={mutating || !!unknown || dialog !== "delete" && !name}>{mutating ? "…" : text.confirm}</button>}</footer>
    </form></div>}
  </section>;
}
