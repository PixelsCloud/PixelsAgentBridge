import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Expand, Keyboard, Maximize2, Minus, Moon, MousePointer2, Sun, X } from "lucide-react";
import brand from "./assets/brand.svg";
import "./windowChrome.css";
import "./remoteDesktop.css";
import type { Assistance } from "./DesktopAssistanceNotice";
import { initialLanguage } from "./i18n";

type Screen = { id: string; name: string; desktop_width: number; desktop_height: number; primary: boolean };
type Quality = { session_id: string; generation: number; capture_fps_milli: number; encoder_target_bps: number | null };
type Status = { phase: string; error?: string; input_ready?: boolean; input_enabled?: boolean; decoder?: string | null; quality?: Quality | null; opened?: { config: { session_id: string; screen: Screen; fps: number; generation: number }; screens: Screen[]; input_supported?: boolean } };
const english = initialLanguage() === "en";
const text = english
  ? { title: "Remote desktop", view: "View only", screen: "Display", retry: "Reconnect", full: "Fullscreen", ready: "Preparing", connecting: "Connecting", reconnecting: "Restoring desktop", switching: "Switching display", viewing: "Connected", failed: "Connection failed", ended: "Disconnected", waiting: "Waiting for remote desktop…" }
  : { title: "远程桌面", view: "仅查看", screen: "显示器", retry: "重新连接", full: "全屏", ready: "准备中", connecting: "正在连接", reconnecting: "正在恢复画面", switching: "正在切换屏幕", viewing: "已连接", failed: "连接失败", ended: "已断开", waiting: "等待远端画面…" };
const inputText = english
  ? { enable: "Enable control", disable: "Stop control", active: "Control enabled" }
  : { enable: "启用操作", disable: "停止操作", active: "可操作" };

export default function RemoteDesktopWindow() {
  const [status, setStatus] = useState<Status>({ phase: "ready" });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [assistance, setAssistance] = useState<Assistance[]>([]);
  const [statisticsExpanded, setStatisticsExpanded] = useState(true);
  const [title, setTitle] = useState(text.title);
  const [dark, setDark] = useState(() => localStorage.getItem("pab.theme") === "dark");
  const appWindow = getCurrentWindow();
  useEffect(() => { void getCurrentWindow().title().then(value => { if (value) setTitle(value); }).catch(() => {}); }, []);
  useEffect(() => { document.documentElement.dataset.theme = dark ? "dark" : "light"; }, [dark]);
  useEffect(() => {
    let active = true; let pending = false;
    const refresh = async () => {
      if (pending) return; pending = true;
      try { const rows = await invoke<Assistance[]>("desktop_viewer_assistance"); if (active) setAssistance(rows); }
      catch { if (active) setAssistance([]); }
      finally { pending = false; }
    };
    void refresh(); const timer = window.setInterval(() => void refresh(), 1000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen<Status>("desktop-viewer-status", ({ payload }) => {
        if (active) { setStatus(payload); setError(""); setBusy(false); }
      });
      if (!active) { unlisten(); return; }
      const initial = await invoke<Status>("desktop_viewer_start");
      if (active) setStatus((current) => current.phase === "ready" ? initial : current);
    })().catch((cause) => { if (active) setError(String(cause)); });
    return () => { active = false; unlisten?.(); };
  }, []);
  useEffect(() => {
    let active = true;
    const subscription = listen<Quality>("desktop-viewer-quality", ({ payload }) => {
      if (active) setStatus(current => current.phase === "viewing"
        && current.opened?.config.session_id === payload.session_id
        && current.opened.config.generation === payload.generation
        ? { ...current, quality: payload } : current);
    });
    return () => { active = false; void subscription.then(unlisten => unlisten()); };
  }, []);
  async function control(arguments_: Record<string, unknown>) {
    setBusy(true); setError("");
    try { await invoke("desktop_viewer_control", arguments_); }
    catch (cause) { setError(String(cause)); setBusy(false); }
  }
  const opened = status.opened;
  const quality = status.quality;
  const degraded = quality && ((quality.encoder_target_bps || 0) > 0
    || quality.capture_fps_milli < (opened?.config.fps || 15) * 1000);
  const budget = quality?.encoder_target_bps
    ? `${(quality.encoder_target_bps / 1_000_000).toFixed(2)} Mbps` : null;
  const qualityText = degraded ? `${english ? "Reduced quality" : "已降档"} · ${budget || `${(quality.capture_fps_milli / 1000).toFixed(1)} FPS`}` : null;
  const phase = text[status.phase as keyof typeof text] || status.phase;
  const decoder = status.decoder === "openh264_software"
    ? (english ? "Software decoding" : "软件解码")
    : ["d3d11_hardware", "videotoolbox_hardware"].includes(status.decoder || "")
      ? (english ? "Hardware decoding" : "硬件解码") : null;
  const statisticsTitle = english ? "Statistics" : "统计信息";
  const captureLimit = `${english ? "Capture limit" : "采集上限"}: ${((quality?.capture_fps_milli ?? (opened?.config.fps || 15) * 1000) / 1000).toFixed(0)} FPS`;
  const statisticsText = [statisticsTitle, `${phase} · ${status.input_enabled ? inputText.active : text.view}`,
    opened && `${opened.config.screen.desktop_width} × ${opened.config.screen.desktop_height} · H.264`,
    decoder, opened && captureLimit, qualityText].filter(Boolean).join("\n");
  useEffect(() => {
    if (status.phase !== "viewing") return;
    // Local native chrome only; a DOM overlay sits below native video surfaces.
    void invoke("desktop_viewer_overlay", { text: statisticsText }).catch(cause => setError(String(cause)));
  }, [statisticsText, status.phase]);
  const windowAction = (action: Promise<unknown>) => { void action.catch(cause => setError(String(cause))); };
  return <main className="remote-desktop">
    <header className="titlebar remote-desktop-toolbar"
      onMouseDown={event => {
        if (event.button !== 0 || event.detail !== 1 || (event.target as HTMLElement).closest("button, select, .remote-desktop-controls, .window-controls")) return;
        event.preventDefault(); windowAction(appWindow.startDragging());
      }}
      onDoubleClick={event => {
        if ((event.target as HTMLElement).closest("button, select, .remote-desktop-controls, .window-controls")) return;
        windowAction(appWindow.toggleMaximize());
      }}>
      <div className="titlebar-identity remote-desktop-title" title={title}><img className="brand-mark" src={brand} alt="" /><strong>{title}</strong></div>
      {error && <span className="remote-desktop-control-error" role="alert" title={error}>{error}</span>}
      {assistance[0] && <AssistanceCompletion key={assistance[0].assistance_id}
        request={assistance[0]} count={assistance.length} viewing={status.phase === "viewing"}
        onComplete={(id) => setAssistance((rows) => rows.filter((row) => row.assistance_id !== id))} />}
      <div className="remote-desktop-controls">
        <select aria-label={text.screen} disabled={busy || status.phase !== "viewing"} value={opened?.config.screen.id || ""} onChange={(event) => void control({ screen: event.target.value })}>
          {!opened && <option value="">{text.screen}</option>}
          {opened?.screens.map((screen) => <option key={screen.id} value={screen.id}>{screen.name} · {screen.desktop_width} × {screen.desktop_height}</option>)}
        </select>
        <select aria-label="FPS" value={opened?.config.fps || 15} disabled={busy || status.phase !== "viewing"} onChange={(event) => void control({ fps: Number(event.target.value) })}>
          <option value={15}>15 FPS</option><option value={30}>30 FPS</option>
        </select>
        <button className="remote-desktop-control-toggle" aria-label={status.input_enabled ? inputText.disable : inputText.enable} title={status.input_enabled ? inputText.disable : inputText.enable} aria-pressed={Boolean(status.input_enabled)} disabled={busy || status.phase !== "viewing" || !status.input_ready} onClick={() => void control({ inputEnabled: !status.input_enabled })}><MousePointer2 size={14} /><span>{status.input_enabled ? inputText.disable : inputText.enable}</span></button>
        <button className="remote-desktop-icon-button" aria-label="Ctrl+Alt+Delete" title="Ctrl+Alt+Delete" disabled={busy || status.phase !== "viewing" || !status.input_enabled} onClick={() => void control({ secureAttention: true })}><Keyboard size={16} /></button>
        <button className="remote-desktop-icon-button" aria-label={text.full} title={text.full} onClick={() => windowAction(appWindow.isFullscreen().then(full => appWindow.setFullscreen(!full)))}><Expand size={15} /></button>
        {status.phase === "failed" && <button disabled={busy} onClick={() => void control({ retry: true })}>{text.retry}</button>}
      </div>
      <div className="window-controls">
        <button className="remote-desktop-theme" aria-label={english ? "Switch theme" : "切换主题"} title={english ? "Switch theme" : "切换主题"} onClick={() => { const next = !dark; localStorage.setItem("pab.theme", next ? "dark" : "light"); setDark(next); }}>{dark ? <Sun /> : <Moon />}</button>
        <button aria-label={english ? "Minimize" : "最小化"} title={english ? "Minimize" : "最小化"} onClick={() => windowAction(appWindow.minimize())}><Minus /></button>
        <button aria-label={english ? "Maximize" : "最大化"} title={english ? "Maximize" : "最大化"} onClick={() => windowAction(appWindow.toggleMaximize())}><Maximize2 /></button>
        <button className="close" aria-label={english ? "Close window" : "关闭窗口"} title={english ? "Close window" : "关闭窗口"} onClick={() => windowAction(appWindow.close())}><X /></button>
      </div>
    </header>
    <section className="remote-desktop-placeholder">
      {(error || status.error) ? <p role="alert">{error || status.error}</p> : <p>{status.phase === "ended" ? text.ended : text.waiting}</p>}
    </section>
    <button className="remote-desktop-statistics" aria-label={statisticsTitle} aria-expanded={statisticsExpanded} onClick={() => setStatisticsExpanded(value => !value)}>
      <strong>{statisticsExpanded ? statisticsTitle : "i"}</strong>
      <div hidden={!statisticsExpanded}>
        <span role="status">{phase}</span><span className="remote-desktop-view-only">{status.input_enabled ? inputText.active : text.view}</span>
        {opened && <span>{opened.config.screen.desktop_width} × {opened.config.screen.desktop_height} · H.264</span>}
        {status.phase === "viewing" && decoder && <span className="remote-desktop-decoder">{decoder}</span>}
        {opened && <span>{captureLimit}</span>}
        {status.phase === "viewing" && qualityText && <span className="remote-desktop-quality" title={english ? "Encoder target, not measured bitrate" : "编码码率目标，非实测码率"}>{qualityText}</span>}
      </div>
    </button>
  </main>;
}

// Completion belongs to a request, not the lifetime of the shared viewer. A
// cancelled/expired request's delayed reply must not disable or error the next.
function AssistanceCompletion({ request, count, viewing, onComplete }: {
  request: Assistance; count: number; viewing: boolean; onComplete: (id: string) => void;
}) {
  const [finishing, setFinishing] = useState(false);
  const [error, setError] = useState("");
  async function complete() {
    setFinishing(true); setError("");
    try {
      await invoke("desktop_viewer_assistance_complete", { id: request.assistance_id });
      onComplete(request.assistance_id);
    } catch (cause) { setError(String(cause)); }
    finally { setFinishing(false); }
  }
  return <>
    {error && <span role="alert" title={error}>{error}</span>}
    <button className="remote-desktop-assistance" disabled={finishing || !viewing}
      title={request.request.expected_action} onClick={() => void complete()}>
      {english ? "Complete assistance" : "完成协助"}{count > 1 ? ` (${count})` : ""}: {request.request.expected_action}
    </button>
  </>;
}
