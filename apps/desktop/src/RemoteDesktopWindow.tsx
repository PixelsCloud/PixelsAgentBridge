import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./remoteDesktop.css";

type Screen = { id: string; name: string; desktop_width: number; desktop_height: number; primary: boolean };
type Status = { phase: string; error?: string; input_ready?: boolean; input_enabled?: boolean; opened?: { config: { screen: Screen; fps: number; generation: number }; screens: Screen[]; input_supported?: boolean } };
const english = localStorage.getItem("pab.language") === "en";
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
  async function control(arguments_: Record<string, unknown>) {
    setBusy(true); setError("");
    try { await invoke("desktop_viewer_control", arguments_); }
    catch (cause) { setError(String(cause)); setBusy(false); }
  }
  const opened = status.opened;
  const phase = text[status.phase as keyof typeof text] || status.phase;
  return <main className="remote-desktop">
    <header className="remote-desktop-toolbar">
      <div className="remote-desktop-status"><strong>{text.title}</strong><span className="remote-desktop-view-only">{status.input_enabled ? inputText.active : text.view}</span><span role={error ? "alert" : "status"} title={error || undefined}>{error || phase}</span></div>
      <div className="remote-desktop-controls">
        <select aria-label={text.screen} disabled={busy || status.phase !== "viewing"} value={opened?.config.screen.id || ""} onChange={(event) => void control({ screen: event.target.value })}>
          {!opened && <option value="">{text.screen}</option>}
          {opened?.screens.map((screen) => <option key={screen.id} value={screen.id}>{screen.name} · {screen.desktop_width} × {screen.desktop_height}</option>)}
        </select>
        <select aria-label="FPS" value={opened?.config.fps || 15} disabled={busy || status.phase !== "viewing"} onChange={(event) => void control({ fps: Number(event.target.value) })}>
          <option value={15}>15 FPS</option><option value={30}>30 FPS</option>
        </select>
        <button aria-pressed={Boolean(status.input_enabled)} disabled={busy || status.phase !== "viewing" || !status.input_ready} onClick={() => void control({ inputEnabled: !status.input_enabled })}>{status.input_enabled ? inputText.disable : inputText.enable}</button>
        <button disabled={busy || status.phase !== "viewing" || !status.input_enabled} onClick={() => void control({ secureAttention: true })}>Ctrl+Alt+Delete</button>
        <button onClick={() => void getCurrentWindow().isFullscreen().then((full) => getCurrentWindow().setFullscreen(!full)).catch((cause) => setError(String(cause)))}>{text.full}</button>
        {status.phase === "failed" && <button disabled={busy} onClick={() => void control({ retry: true })}>{text.retry}</button>}
      </div>
    </header>
    <section className="remote-desktop-placeholder">
      {(error || status.error) ? <p role="alert">{error || status.error}</p> : <p>{status.phase === "ended" ? text.ended : text.waiting}</p>}
    </section>
  </main>;
}
