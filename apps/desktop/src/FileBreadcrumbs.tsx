import { useLayoutEffect, useRef, useState } from "react";
import { ChevronRight } from "lucide-react";
import { nativePathKey, type NativePath } from "./fileManagerApi";

type Crumb = { label: string; path: NativePath };
export function FileBreadcrumbs({ items, label, disabled, onNavigate, pathTitle, moreLabel }: {
  items: Crumb[]; label: string; disabled: boolean; onNavigate: (path: NativePath) => void;
  pathTitle: (path: NativePath) => string; moreLabel: string;
}) {
  const box = useRef<HTMLDivElement>(null), measure = useRef<HTMLDivElement>(null);
  const more = useRef<HTMLButtonElement>(null);
  const [tail, setTail] = useState(items.length), [open, setOpen] = useState(false);
  useLayoutEffect(() => {
    setOpen(false);
    const fit = () => {
      const widths = [...(measure.current?.children || [])].map(item => item.getBoundingClientRect().width);
      const available = box.current?.clientWidth || 0;
      if (widths.reduce((sum, width) => sum + width, 0) <= available) { setTail(items.length); return; }
      let count = 1, used = (widths[0] || 0) + (widths[widths.length - 1] || 0) + 44;
      while (count < items.length - 2 && used + widths[items.length - count - 1] <= available) used += widths[items.length - ++count];
      setTail(count);
    };
    fit(); const observer = new ResizeObserver(fit);
    if (box.current) observer.observe(box.current);
    return () => observer.disconnect();
  }, [items]);
  useLayoutEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!box.current?.contains(event.target as Node)) setOpen(false); };
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") { setOpen(false); more.current?.focus(); } };
    document.addEventListener("pointerdown", close); document.addEventListener("keydown", key);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", key); };
  }, [open]);
  const collapsed = items.length > 2 && tail < items.length;
  const hidden = collapsed ? items.slice(1, -tail) : [];
  const crumb = (item: Crumb, index: number) => <span key={nativePathKey(item.path)} className={index === items.length - 1 ? "fm-crumb-current" : undefined}>
    {index > 0 && <ChevronRight size={13}/>}
    <button disabled={disabled} title={pathTitle(item.path)} aria-current={index === items.length - 1 ? "location" : undefined} onClick={() => onNavigate(item.path)}>{item.label}</button>
  </span>;
  return <div ref={box} className="fm-breadcrumb-wrap">
    <div className="fm-crumb-measure" aria-hidden="true"><div ref={measure}>{items.map((item, index) => <span key={index}>{index > 0 && <ChevronRight size={13}/>}<span>{item.label}</span></span>)}</div></div>
    <nav className="fm-breadcrumbs" aria-label={label}>
      {collapsed ? <>{crumb(items[0], 0)}<span className="fm-crumb-more"><ChevronRight size={13}/><button ref={more} disabled={disabled} aria-label={moreLabel} aria-expanded={open} onClick={() => setOpen(value => !value)}>…</button></span>{items.slice(-tail).map((item, index) => crumb(item, items.length - tail + index))}</> : items.map(crumb)}
    </nav>
    {open && <div className="fm-crumb-menu" aria-label={moreLabel}>{hidden.map(item => <button key={nativePathKey(item.path)} title={pathTitle(item.path)} onClick={() => { setOpen(false); onNavigate(item.path); }}>{item.label}</button>)}</div>}
  </div>;
}
