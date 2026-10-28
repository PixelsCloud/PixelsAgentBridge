import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronLeft, ChevronRight, X } from "lucide-react";
import { displayFileText, fileSize, pathLabel, type NativePath } from "./fileManagerApi";
import { initialLanguage } from "./i18n";

type MetadataResult = { requested: { modified_at_unix_ms: number | null; unix_mode: number | null }; actual: { modified_at_unix_ms: number | null; unix_mode: number | null }; warnings: { attribute: "modified_time" | "unix_mode"; code: string; message: string }[] };
type Item = { spec: { id: string; relative_name: string; source: NativePath; destination: NativePath; size: string }; effective_destination?: NativePath; state: string; confirmed: string; message: string | null; attempt: string | null; metadata_result?: MetadataResult | null };
const en = initialLanguage() === "en";
const states: Record<string, string> = en ? { queued: "Queued", preparing: "Preparing", waiting_conflict: "Name conflict", waiting_children: "Waiting for child items", transferring: "Transferring", verifying: "Verifying", publishing: "Publishing", pause_requested: "Pausing…", paused: "Paused", cancel_requested: "Cancelling…", cancelled: "Cancelled", completed: "Completed", skipped: "Skipped", interrupted: "Interrupted", reconciling: "Reconciling", unconfirmed: "Unconfirmed", failed: "Failed" } : { queued: "排队中", preparing: "准备中", waiting_conflict: "名称冲突", waiting_children: "等待子项完成", transferring: "传输中", verifying: "校验中", publishing: "发布中", pause_requested: "暂停中…", paused: "已暂停", cancel_requested: "取消中…", cancelled: "已取消", completed: "已完成", skipped: "已跳过", interrupted: "已中断", reconciling: "正在核实", unconfirmed: "待确认", failed: "失败" };

export function FileBatchDialog({ id, initialConflict, disabled, onClose, onChange }: { id: string; initialConflict: boolean; disabled: boolean; onClose: () => void; onChange: () => void }) {
  const [filter, setFilter] = useState(initialConflict ? "waiting_conflict" : ""), [after, setAfter] = useState(0);
  const [items, setItems] = useState<Item[]>([]), [error, setError] = useState(""), [loading, setLoading] = useState(false), [pending, setPending] = useState<string | null>(null), [refresh, setRefresh] = useState(0);
  const [applyRemaining, setApplyRemaining] = useState(false);
  const [readError, setReadError] = useState("");
  const dialog = useRef<HTMLDivElement>(null), changing = useRef(false), changed = useRef(onChange), closed = useRef(onClose);
  changed.current = onChange; closed.current = onClose;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    dialog.current?.querySelector<HTMLElement>("button")?.focus();
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closed.current(); }
      if (event.key !== "Tab") return;
      const targets = [...(dialog.current?.querySelectorAll<HTMLElement>("button:enabled,input:enabled,select:enabled,[tabindex='0']") || [])], index = targets.indexOf(document.activeElement as HTMLElement);
      if (event.shiftKey && index <= 0) { event.preventDefault(); targets[targets.length - 1]?.focus(); }
      else if (!event.shiftKey && (index < 0 || index === targets.length - 1)) { event.preventDefault(); targets[0]?.focus(); }
    };
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("keydown", key); if (previous?.isConnected) previous.focus(); };
  }, []);
  useEffect(() => {
    let alive = true, first = true, timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      if (!changing.current && !disabled) {
        if (first) { setLoading(true); first = false; }
        try { const values = await invoke<Item[]>("file_manager_batch_items", { id, after, filter: filter || null }); if (alive && !changing.current) { setItems(values); setReadError(""); } }
        catch (cause) { if (alive) setReadError(String(cause)); }
        finally { if (alive) setLoading(false); }
      }
      if (alive) timer = setTimeout(() => void poll(), 1000);
    };
    void poll(); return () => { alive = false; clearTimeout(timer); };
  }, [id, after, filter, refresh, disabled]);
  async function resolve(item: Item, choice: string) {
    if (changing.current || disabled) return;
    changing.current = true; setPending(item.spec.id); setError("");
    try {
      await invoke("file_manager_resolve_conflict", { batch: id, item: item.spec.id, choice, applyRemaining });
      changed.current();
      if (filter === "waiting_conflict") {
        const remaining = await invoke<Item[]>("file_manager_batch_items", { id, after: 0, filter: "waiting_conflict" });
        if (remaining.length === 0) { closed.current(); return; }
        setAfter(0); setItems(remaining);
      }
    }
    catch (cause) { setError(String(cause)); }
    finally { changing.current = false; setPending(null); setRefresh(value => value + 1); }
  }
  return <div className="fm-modal-shade"><div ref={dialog} role="dialog" aria-modal="true" aria-label={en ? "Transfer details" : "传输详情"} className="fm-dialog fm-batch-dialog">
    <header><strong>{en ? "Transfer details" : "传输详情"}</strong><button title={en ? "Close details" : "关闭详情"} onClick={onClose}><X size={18} /></button></header>
    <div className="fm-batch-filter"><label>{en ? "Status" : "状态"}<select aria-label={en ? "Item status" : "项目状态"} value={filter} onChange={event => { setItems([]); setAfter(0); setFilter(event.target.value); }}><option value="">{en ? "All" : "全部"}</option>{Object.entries(states).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label><span aria-live="polite">{loading ? en ? "Updating…" : "正在更新…" : ""}</span></div>
    {items.some(item => item.state === "waiting_conflict") && <label className="fm-conflict-remaining"><input type="checkbox" disabled={disabled || pending !== null} checked={applyRemaining} onChange={event => setApplyRemaining(event.target.checked)}/>{en ? "Apply this choice to all remaining conflicts in this transfer" : "对本次传输后续所有冲突应用此选择"}</label>}
    {(error || readError) && <div role="alert" className="fm-error">{error || readError}</div>}
    <div className="fm-batch-items" tabIndex={0}>{items.length === 0 ? <p className="fm-empty">{en ? "No items in this view" : "当前没有匹配的项目"}</p> : items.map(item => <article key={item.spec.id} className="fm-batch-item">
      <div><strong>{displayFileText(item.spec.relative_name)}</strong><span>{item.state === "completed" && !!item.metadata_result?.warnings.length ? en ? "Completed with attribute warnings" : "已完成，部分属性未保留" : states[item.state] || item.state}</span></div>
      <small>{fileSize(item.confirmed)} / {fileSize(item.spec.size)}</small>
      <dl><dt>{en ? "Source" : "源路径"}</dt><dd>{pathLabel(item.spec.source)}</dd><dt>{en ? "Destination" : "目标路径"}</dt><dd>{pathLabel(item.effective_destination || item.spec.destination)}</dd>{item.attempt && <><dt>{en ? "Request" : "请求 ID"}</dt><dd>{item.attempt}</dd></>}</dl>
      {item.message && <p className={item.state === "failed" ? "fm-item-error" : ""}>{displayFileText(item.message)}</p>}
      {!!item.metadata_result?.warnings.length && <div className="fm-attribute-warnings" role="note">{item.metadata_result.warnings.map(warning => <p key={warning.attribute}>
        <strong>{warning.attribute === "modified_time" ? en ? "Modified time" : "修改时间" : en ? "Permissions" : "文件权限"}：</strong>
        {warning.code === "unsupported" ? en ? "Not supported by the destination filesystem; file contents were saved." : "目标文件系统不支持此属性；文件内容已保存。" : displayFileText(warning.message)}
      </p>)}</div>}
      {item.state === "waiting_conflict" && <div className="fm-conflict-actions">{([ ["replace", en ? "Replace" : "替换"], ["skip", en ? "Skip" : "跳过"], ["keep_both", en ? "Keep both" : "保留两者"] ]).map(([choice, label]) => <button key={choice} disabled={disabled || pending !== null} onClick={() => void resolve(item, choice)}>{pending === item.spec.id ? "…" : label}</button>)}</div>}
    </article>)}</div>
    <footer><span>{after + (items.length ? 1 : 0)}–{after + items.length}</span><button title={en ? "Previous items" : "上一页项目"} disabled={after === 0 || pending !== null || loading} onClick={() => { setItems([]); setAfter(value => Math.max(0, value - 128)); }}><ChevronLeft size={16} /></button><button title={en ? "Next items" : "下一页项目"} disabled={items.length < 128 || pending !== null || loading} onClick={() => { setItems([]); setAfter(value => value + 128); }}><ChevronRight size={16} /></button></footer>
  </div></div>;
}
