import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Bug, Expand, Maximize2, Minus, Moon, MousePointer2, Shrink, Sun, X } from "lucide-react";
import brand from "./assets/brand.svg";
import "./windowChrome.css";
import "./remoteDesktop.css";
import type { Assistance } from "./DesktopAssistanceNotice";
import { initialLanguage } from "./i18n";
import { useViewerFullscreen } from "./useViewerFullscreen";

type Screen = { id: string; name: string; desktop_width: number; desktop_height: number; primary: boolean };
type Quality = { session_id: string; generation: number; capture_fps_milli: number; encoder_target_bps: number | null };
type Status = {
  phase: string; error?: string; input_ready?: boolean; input_enabled?: boolean; frame_ready?: boolean;
  decoder?: string | null; quality?: Quality | null; remote_os?: string | null; connection_path?: string | null;
  opened?: { config: { session_id: string; screen: Screen; fps: number; generation: number }; screens: Screen[]; input_supported?: boolean };
};
const english = initialLanguage() === "en";
const text = english
  ? { title: "Remote desktop", view: "View only", screen: "Display", retry: "Reconnect", full: "Fullscreen", exitFull: "Exit fullscreen (Esc)", ready: "Preparing", connecting: "Connecting", reconnecting: "Restoring desktop", switching: "Switching display", viewing: "Connected", failed: "Connection failed", ended: "Disconnected", waiting: "Waiting for remote desktop…" }
  : { title: "远程桌面", view: "仅查看", screen: "显示器", retry: "重新连接", full: "全屏", exitFull: "退出全屏（Esc）", ready: "准备中", connecting: "正在连接", reconnecting: "正在恢复画面", switching: "正在切换屏幕", viewing: "已连接", failed: "连接失败", ended: "已断开", waiting: "等待远端画面…" };
const inputText = english
  ? { enable: "Enable control", disable: "Stop control", active: "Control enabled" }
  : { enable: "启用操作", disable: "停止操作", active: "可操作" };

export default function RemoteDesktopWindow() {
  const [status, setStatus] = useState<Status>({ phase: "ready" });
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [assistance, setAssistance] = useState<Assistance[]>([]);
  const [statisticsExpanded, setStatisticsExpanded] = useState(false);
  const [title, setTitle] = useState(text.title);
  const [dark, setDark] = useState(() => localStorage.getItem("pab.theme") === "dark");
  const closePending = useRef(false);
  const { appWindow, fullscreen, toggleFullscreen } = useViewerFullscreen(setError);
  async function requestClose() {
    if (closePending.current) return;
    closePending.current = true;
    setError("");
    try { await invoke("desktop_viewer_close", { english }); }
    catch (cause) { setError(String(cause)); }
    finally { closePending.current = false; }
  }
  useEffect(() => {
    let active = true;
    const subscription = listen("desktop-viewer-close-requested", () => { if (active) void requestClose(); });
    return () => { active = false; void subscription.then(unlisten => unlisten()); };
  }, []);
  useEffect(() => { void appWindow.title().then(value => { if (value) setTitle(value); }).catch(() => {}); }, [appWindow]);
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
      if (active) setStatus(current => current.phase === "ready" ? initial : current);
    })().catch(cause => { if (active) setError(String(cause)); });
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
  const quality = status.phase === "viewing" ? status.quality : null;
  const phase = text[status.phase as keyof typeof text] || status.phase;
  const unknown = english ? "Not reported" : "未上报";
  const decoder = status.decoder === "openh264_software"
    ? (english ? "Software decoding" : "软件解码")
    : ["d3d11_hardware", "videotoolbox_hardware"].includes(status.decoder || "")
      ? (english ? "Hardware decoding" : "硬件解码") : null;
  const statisticsTitle = english ? "Debug information" : "调试信息";
  const budget = quality?.encoder_target_bps == null ? unknown : quality.encoder_target_bps === 0
    ? (english ? "Full quality" : "全质量") : `${(quality.encoder_target_bps / 1_000_000).toFixed(2)} Mbps`;
  const diagnostics = [
    [english ? "State" : "连接状态", `${phase} · ${status.input_enabled ? inputText.active : text.view}`],
    [english ? "Remote OS" : "远端系统", status.remote_os || unknown],
    [english ? "Connection path" : "连接路径", status.connection_path === "p2p" ? "P2P" : status.connection_path === "relay" ? "Relay" : unknown],
    [text.screen, opened ? `${opened.config.screen.name.slice(0, 80)} (${opened.screens.length})` : unknown],
    [english ? "Resolution / codec" : "分辨率／编码", opened ? `${opened.config.screen.desktop_width} × ${opened.config.screen.desktop_height} · H.264` : unknown],
    [english ? "Decoder" : "解码器", status.phase === "viewing" ? [decoder, status.decoder].filter(Boolean).join(" · ") || unknown : unknown],
    [english ? "Requested limit" : "请求帧率上限", opened ? `${opened.config.fps} FPS` : unknown],
    [english ? "Capture limit" : "采集上限", quality ? `${(quality.capture_fps_milli / 1000).toFixed(1)} FPS` : unknown],
    [english ? "Encoder target (not measured)" : "编码目标码率（非实测）", budget],
    [english ? "First frame / input ready" : "首帧／输入就绪", `${status.frame_ready ? "✓" : "—"} / ${status.input_ready ? "✓" : "—"}`],
    [english ? "Input supported" : "远端输入支持", opened ? (opened.input_supported ? "✓" : "—") : unknown],
    [english ? "Generation" : "会话代次", opened ? String(opened.config.generation) : unknown],
    [english ? "Session" : "会话标识", opened?.config.session_id || unknown],
  ];
  const statisticsText = [statisticsTitle, ...diagnostics.map(([label, value]) => `${label}: ${value}`)].join("\n");
  useEffect(() => {
    if (status.phase !== "viewing") return;
    // Native video covers DOM panels. Visibility belongs to this titlebar button.
    void invoke("desktop_viewer_overlay", { text: statisticsExpanded ? statisticsText : "" })
      .catch(cause => setError(String(cause)));
  }, [statisticsText, statisticsExpanded, status.phase]);
  const windowAction = (action: Promise<unknown>) => { void action.catch(cause => setError(String(cause))); };

  return <main className={`remote-desktop${fullscreen ? " remote-desktop-fullscreen" : ""}`}>
    <header className="titlebar remote-desktop-toolbar" hidden={fullscreen}
      onMouseDown={event => {
        if (event.button !== 0 || event.detail !== 1 || (event.target as HTMLElement).closest("button, select, .remote-desktop-controls, .window-controls")) return;
        event.preventDefault(); windowAction(appWindow.startDragging());
      }}
      onDoubleClick={event => {
        if ((event.target as HTMLElement).closest("button, select, .remote-desktop-controls, .window-controls")) return;
        windowAction(appWindow.toggleMaximize());
      }}>
      <div className="titlebar-identity remote-desktop-title" title={title}><img className="brand-mark" src={brand} alt="" /><strong>{title}</strong></div>
      <span className="remote-desktop-phase" role="status">{phase}</span>
      {error && <span className="remote-desktop-control-error" role="alert" title={error}>{error}</span>}
      {assistance[0] && <AssistanceCompletion key={assistance[0].assistance_id}
        request={assistance[0]} count={assistance.length} viewing={status.phase === "viewing"}
        onComplete={id => setAssistance(rows => rows.filter(row => row.assistance_id !== id))} />}
      <div className="remote-desktop-controls">
        <select aria-label={text.screen} disabled={busy || status.phase !== "viewing"} value={opened?.config.screen.id || ""} onChange={event => void control({ screen: event.target.value })}>
          {!opened && <option value="">{text.screen}</option>}
          {opened?.screens.map(screen => <option key={screen.id} value={screen.id}>{screen.name} · {screen.desktop_width} × {screen.desktop_height}</option>)}
        </select>
        <select aria-label="FPS" value={opened?.config.fps || 15} disabled={busy || status.phase !== "viewing"} onChange={event => void control({ fps: Number(event.target.value) })}>
          <option value={15}>15 FPS</option><option value={30}>30 FPS</option>
        </select>
        <button className="remote-desktop-control-toggle" aria-label={status.input_enabled ? inputText.disable : inputText.enable} title={status.input_enabled ? inputText.disable : inputText.enable} aria-pressed={Boolean(status.input_enabled)} disabled={busy || status.phase !== "viewing" || !status.input_ready || !opened?.input_supported} onClick={() => void control({ inputEnabled: !status.input_enabled })}><MousePointer2 size={14} /><span>{status.input_enabled ? inputText.disable : inputText.enable}</span></button>
        {status.remote_os === "windows" && <button className="remote-desktop-sas" disabled={busy || status.phase !== "viewing" || !status.input_enabled} onClick={() => void control({ secureAttention: true })}>Ctrl + Alt + Delete</button>}
        <button className="remote-desktop-icon-button" aria-label={statisticsTitle} title={statisticsTitle} aria-expanded={statisticsExpanded} aria-pressed={statisticsExpanded} onClick={() => setStatisticsExpanded(value => !value)}><Bug size={16} /></button>
        <button className="remote-desktop-icon-button" aria-label={fullscreen ? text.exitFull : text.full} title={fullscreen ? text.exitFull : text.full} aria-pressed={fullscreen} onClick={() => void toggleFullscreen()}>{fullscreen ? <Shrink size={15} /> : <Expand size={15} />}</button>
        {status.phase === "failed" && <button disabled={busy} onClick={() => void control({ retry: true })}>{text.retry}</button>}
      </div>
      <div className="window-controls">
        <button className="remote-desktop-theme" aria-label={english ? "Switch theme" : "切换主题"} title={english ? "Switch theme" : "切换主题"} onClick={() => { const next = !dark; localStorage.setItem("pab.theme", next ? "dark" : "light"); setDark(next); }}>{dark ? <Sun /> : <Moon />}</button>
        <button aria-label={english ? "Minimize" : "最小化"} title={english ? "Minimize" : "最小化"} onClick={() => windowAction(appWindow.minimize())}><Minus /></button>
        <button aria-label={english ? "Maximize" : "最大化"} title={english ? "Maximize" : "最大化"} onClick={() => windowAction(appWindow.toggleMaximize())}><Maximize2 /></button>
        <button className="close" aria-label={english ? "Close window" : "关闭窗口"} title={english ? "Close window" : "关闭窗口"} onClick={() => void requestClose()}><X /></button>
      </div>
    </header>
    <section className="remote-desktop-placeholder">
      {(error || status.error) ? <p role="alert">{error || status.error}</p> : <p>{status.phase === "ended" ? text.ended : text.waiting}</p>}
    </section>
    <aside className="remote-desktop-statistics" aria-label={statisticsTitle} hidden={!statisticsExpanded}>
      <strong>{statisticsTitle}</strong><dl>{diagnostics.map(([label, value]) => <div key={label}><dt>{label}</dt><dd
        className={label === (english ? "Decoder" : "解码器") ? "remote-desktop-decoder" : undefined}>{value}</dd></div>)}</dl>
      <small>{english ? "Limits and encoder targets are not measured FPS or bitrate." : "帧率上限和编码目标不代表实际帧率或实测码率。"}</small>
    </aside>
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
