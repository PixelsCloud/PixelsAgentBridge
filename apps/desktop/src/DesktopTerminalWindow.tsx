import { userFacingError } from "./userFacingError";
import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Alert, App, Button, ConfigProvider, Select, Space, Tag, theme } from "antd";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { Copy, Clipboard, Eraser, RefreshCw, Square, Minus, Maximize2, X } from "lucide-react";
import brand from "./assets/brand.svg";
import "@xterm/xterm/css/xterm.css";
import "./windowChrome.css";
import "./desktopTerminal.css";
import { desktopTerminalText } from "./desktopTerminalText";
import { initialLanguage } from "./i18n";
import type { ExecutionEntry, ExecutionReply, ExecutionSelection } from "./executionQueries";
import type { ExecutionIdentity } from "./operatorTypes";

type Opened = { session_id: string; shell: string; arguments: string[]; cwd: string; identity: ExecutionIdentity; cols: number; rows: number; connection_path: string; os_family?: "windows" | "linux" | "macos" | "android" };
function terminalError(cause: unknown): string {
  const message = String(cause);
  if (/Android does not provide a System\/root terminal|android_system_action_unavailable|android_pty_start_failed|android_terminal_requires_app_uid/.test(message))
    return language === "en" ? "This feature is not available on this Android device." : "此 Android 设备暂无此功能。";
  return message;
}
type LaunchMode = "console" | "system" | null;
const language = initialLanguage();
const text = desktopTerminalText[language === "en" ? "en" : "zh-CN"];
const dark = localStorage.getItem("pab.theme") === "dark";

export default function DesktopTerminalWindow() {
  return <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, token: { colorPrimary: "#218574", fontSize: 13, borderRadius: 7 } }}><App><TerminalContent /></App></ConfigProvider>;
}
function TerminalContent() {
  const { modal, message } = App.useApp();
  const appWindow = getCurrentWindow();
  const host = useRef<HTMLDivElement>(null);
  const terminal = useRef<Terminal | null>(null);
  const fit = useRef<FitAddon | null>(null);
  const live = useRef(false);
  const mustEnd = useRef(false);
  const generation = useRef(0);
  const sessionId = useRef<string | null>(null);
  const receivedOffset = useRef(0);
  const closeBusy = useRef(false);
  const queue = useRef<Uint8Array[]>([]);
  const queuedBytes = useRef(0);
  const draining = useRef(false);
  const pendingAck = useRef(0);
  const acked = useRef(0);
  const ackBusy = useRef(false);
  const pendingResize = useRef<{ cols: number; rows: number } | null>(null);
  const resizeBusy = useRef(false);
  const pasteBusy = useRef(false);
  const [code, setCode] = useState("");
  const [launchMode, setLaunchMode] = useState<LaunchMode>(null);
  const automaticStarted = useRef(false);
  const automaticBusy = useRef(false);
  const [accounts, setAccounts] = useState<ExecutionEntry[]>([]);
  const [selected, setSelected] = useState("");
  const [loadingAccounts, setLoadingAccounts] = useState(false);
  const [phase, setPhase] = useState<"preparing" | "ready" | "starting" | "running" | "ended">("preparing");
  const [opened, setOpened] = useState<Opened | null>(null);
  const [privacyHintDismissed, setPrivacyHintDismissed] = useState(false);
  const [error, setError] = useState("");
  const [reason, setReason] = useState("");
  const stateRef = useRef(phase); stateRef.current = phase;

  async function refreshAccounts() {
    setLoadingAccounts(true); setError(""); setSelected("");
    try {
      const reply = await invoke<ExecutionReply>("desktop_terminal_accounts");
      if (reply.state !== "completed" || reply.data?.type !== "execution_contexts") throw new Error(reply.error || text.noAccounts);
      setAccounts(reply.data.entries.filter((entry) => entry.mode === "user" || entry.mode === "service"));
      const previous = localStorage.getItem(`pab.desktopTerminal.account.${code}`);
      const account = reply.data.entries.find((entry) => entry.mode === "user" && entry.account_id === previous && entry.selection && "context_ref" in entry.selection);
      if (previous === "service") setSelected("service");
      else if (account?.selection && "context_ref" in account.selection) setSelected(account.selection.context_ref);
      setPhase("ready");
    } catch (cause) { setError(terminalError(cause)); setPhase("ready"); }
    finally { setLoadingAccounts(false); }
  }
  async function consume() {
    if (ackBusy.current) return;
    ackBusy.current = true;
    const gen = generation.current;
    const id = sessionId.current;
    try {
      while (gen === generation.current && pendingAck.current > acked.current) {
        const offset = pendingAck.current;
        await invoke("desktop_terminal_consume", { sessionId: id, offset });
        if (gen !== generation.current) break;
        acked.current = offset;
      }
    } catch (cause) { if (live.current && gen === generation.current) { setError(terminalError(cause)); live.current = false; setPhase("ended"); } }
    finally { ackBusy.current = false; if (live.current && pendingAck.current > acked.current && gen !== generation.current) void consume(); }
  }
  async function drainInput() {
    if (draining.current) return;
    draining.current = true;
    const gen = generation.current;
    const id = sessionId.current;
    try {
      while (live.current && gen === generation.current && queue.current.length) {
        const bytes = queue.current.shift()!;
        queuedBytes.current -= bytes.length;
        await invoke("desktop_terminal_input", { sessionId: id, bytes: Array.from(bytes) });
      }
    } catch (cause) {
      if (gen === generation.current) { setError(terminalError(cause)); live.current = false; setPhase("ended"); queue.current = []; queuedBytes.current = 0; }
    } finally { draining.current = false; if (live.current && queue.current.length && gen !== generation.current) void drainInput(); }
  }
  function input(data: string) {
    if (!live.current) return;
    (window as unknown as { __PIXELS_TERMINAL_ACCEPTANCE__?: { onInput?: (data: string) => void } }).__PIXELS_TERMINAL_ACCEPTANCE__?.onInput?.(data);
    const bytes = new TextEncoder().encode(data);
    if (queuedBytes.current + bytes.length > 64 * 1024) { void message.warning(text.queueFull); return; }
    for (let offset = 0; offset < bytes.length; offset += 4096) queue.current.push(bytes.slice(offset, offset + 4096));
    queuedBytes.current += bytes.length; void drainInput();
  }
  async function paste(data: string) {
    const bytes = new TextEncoder().encode(data);
    if (!live.current || pasteBusy.current) return;
    if (bytes.length > 256 * 1024) { void message.warning(text.pasteLarge); return; }
    if (/\r|\n/.test(data) && !terminal.current?.modes.bracketedPasteMode) {
      const confirmed = await modal.confirm({ title: text.pasteTitle, content: text.pasteBody, okText: text.pasteConfirm, cancelText: text.cancel });
      if (!confirmed) return;
    }
    pasteBusy.current = true;
    const gen = generation.current;
    try {
      // Only one paste source (256 KiB) and a 64 KiB input queue are retained.
      // Use xterm's paste path so bracketed-paste escape sequences are preserved.
      for (let offset = 0; offset < bytes.length && live.current && gen === generation.current; ) {
        // Leave room for xterm's bracketed-paste escape sequences too.
        if (queuedBytes.current > 48 * 1024 - 64) { await new Promise((resolve) => setTimeout(resolve, 10)); continue; }
        let end = Math.min(bytes.length, offset + 16 * 1024);
        while (end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--;
        terminal.current?.paste(new TextDecoder().decode(bytes.subarray(offset, end))); offset = end;
      }
    } finally { pasteBusy.current = false; }
  }
  async function resize() {
    if (resizeBusy.current || !live.current) return;
    resizeBusy.current = true;
    try {
      while (pendingResize.current && live.current) {
        const size = pendingResize.current; pendingResize.current = null;
        await invoke("desktop_terminal_resize", { sessionId: sessionId.current, ...size });
      }
    } catch (cause) { if (live.current) setError(terminalError(cause)); }
    finally { resizeBusy.current = false; }
  }
  async function endSession() {
    await invoke("desktop_terminal_end", { sessionId: sessionId.current });
    sessionId.current = null;
    mustEnd.current = false;
    live.current = false; generation.current++; queue.current = []; queuedBytes.current = 0;
    setPhase("ended"); setReason(text.ended);
  }
  async function requestClose(closeWindow: boolean) {
    if (closeBusy.current) return;
    closeBusy.current = true;
    try {
      if (stateRef.current === "starting") return;
      if (live.current || mustEnd.current) {
        const confirmed = await modal.confirm({ title: text.closeTitle, content: text.closeBody, okText: text.confirm, cancelText: text.cancel, okButtonProps: { danger: true } });
        if (!confirmed) return;
      }
      await endSession();
      if (closeWindow) await invoke("desktop_terminal_destroy");
    } catch (cause) { setError(terminalError(cause)); }
    finally { closeBusy.current = false; }
  }
  useEffect(() => {
    document.documentElement.dataset.theme = dark ? "dark" : "light";
    let disposed = false;
    void invoke<{ code: string; launch_mode: LaunchMode }>("desktop_terminal_info").then((value) => { if (!disposed) { setCode(value.code); setLaunchMode(value.launch_mode); } }).catch((cause) => { if (!disposed) setError(terminalError(cause)); });
    const term = new Terminal({ scrollback: 10000, fontSize: 14, fontFamily: '"Cascadia Mono", "SFMono-Regular", Consolas, monospace', cursorBlink: true, theme: { background: "#142334", foreground: "#e7f3f1", cursor: "#8cd6c9", selectionBackground: "#21857470" } });
    const addon = new FitAddon(); term.loadAddon(addon); term.open(host.current!); terminal.current = term; fit.current = addon;
    const data = term.onData(input);
    const observer = new ResizeObserver(() => {
      addon.fit();
      pendingResize.current = { cols: Math.max(20, Math.min(500, term.cols)), rows: Math.max(5, Math.min(200, term.rows)) }; void resize();
    }); observer.observe(host.current!); addon.fit();
    const capturePaste = (event: ClipboardEvent) => { event.preventDefault(); event.stopImmediatePropagation(); void paste(event.clipboardData?.getData("text/plain") || ""); };
    host.current!.addEventListener("paste", capturePaste, true);
    const unlisten = getCurrentWindow().onCloseRequested((event) => { event.preventDefault(); void requestClose(true); });
    const unlistenAccount = listen<string>("desktop-terminal-account-ended", (event) => { live.current = false; setPhase("ended"); setError(event.payload); });
    return () => { disposed = true; void unlisten.then((fn) => fn()); void unlistenAccount.then((fn) => fn()); observer.disconnect(); data.dispose(); host.current?.removeEventListener("paste", capturePaste, true); term.dispose(); terminal.current = null; };
  }, []);
  useEffect(() => {
    if (!code) return;
    if (launchMode) {
      if (!automaticStarted.current) { automaticStarted.current = true; void automaticStart(); }
    } else void refreshAccounts();
  }, [code, launchMode]);
  async function automaticStart() {
    if (automaticBusy.current) return;
    automaticBusy.current = true;
    const gen = generation.current;
    setPhase("preparing"); setError(""); setReason("");
    try {
      const selection = await invoke<ExecutionSelection>("desktop_terminal_selection");
      if (generation.current !== gen) return;
      await start(selection);
    } catch (cause) { if (generation.current === gen) { setError(terminalError(cause)); setPhase("ended"); } }
    finally { automaticBusy.current = false; }
  }
  async function start(requestedSelection?: ExecutionSelection) {
    const selection: ExecutionSelection | undefined = requestedSelection ?? (selected === "service" ? { mode: "service" } : accounts.find((entry) => entry.selection && "context_ref" in entry.selection && entry.selection.context_ref === selected)?.selection ?? undefined);
    if (!selection || !terminal.current) return;
    if (mustEnd.current && !live.current) {
      const confirmed = await modal.confirm({ title: text.closeTitle, content: text.closeBody, okText: text.confirm, cancelText: text.cancel });
      if (!confirmed) { setPhase("ended"); return; }
    }
    setPhase("starting"); setError(""); setReason("");
    const gen = ++generation.current; pendingAck.current = 0; acked.current = 0; receivedOffset.current = 0; queue.current = []; queuedBytes.current = 0;
    terminal.current.reset(); fit.current?.fit();
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buffer) => {
      if (gen !== generation.current) return;
      const fail = (reason: string) => {
        if (gen !== generation.current) return;
        const failedGeneration = ++generation.current; live.current = false; queue.current = []; queuedBytes.current = 0;
        setError(reason); setPhase("ended");
        const id = sessionId.current;
        void invoke("desktop_terminal_end", { sessionId: id }).then(() => {
          if (generation.current === failedGeneration && sessionId.current === id) { sessionId.current = null; mustEnd.current = false; }
        }).catch((cause) => { if (generation.current === failedGeneration) setError(`${reason}; ${String(cause)}`); });
      };
      const acceptance = (window as unknown as { __PIXELS_TERMINAL_ACCEPTANCE__?: { onFrame: ((frame: ArrayBuffer) => void) | null; paused: boolean; pending: (() => void)[]; pendingBytes: number; maxPendingBytes: number } }).__PIXELS_TERMINAL_ACCEPTANCE__;
      acceptance?.onFrame?.(buffer);
      const apply = () => {
      if (gen !== generation.current) return;
      const frame = new Uint8Array(buffer);
      if (frame.length < 9) { fail("incomplete terminal IPC frame"); return; }
      const rawOffset = new DataView(buffer).getBigUint64(1);
      if (rawOffset > BigInt(Number.MAX_SAFE_INTEGER)) { fail("terminal offset exceeds supported precision"); return; }
      const offset = Number(rawOffset);
      if (frame[0] === 0) {
        const bytes = frame.subarray(9);
        if (offset !== receivedOffset.current) { fail("terminal output gap or duplicate detected; no data was silently skipped"); return; }
        receivedOffset.current = offset + bytes.length;
        live.current = true;
        terminal.current?.write(bytes, () => { if (gen !== generation.current) return; pendingAck.current = offset + bytes.length; void consume(); });
      } else if (frame[0] === 1) {
        let event: { reason: string; exit_code: number | null };
        try { event = JSON.parse(new TextDecoder().decode(frame.subarray(9))); }
        catch { fail("invalid terminal ending frame"); return; }
        if (typeof event.reason !== "string" || (event.exit_code !== null && !Number.isInteger(event.exit_code))) { fail("invalid terminal ending fields"); return; }
        live.current = false; setPhase("ended");
        if (event.exit_code !== null) mustEnd.current = false;
        setReason(`${userFacingError(event.reason, language)}${event.exit_code === null ? "" : ` · ${event.exit_code}`}`);
      } else fail("invalid terminal IPC frame kind");
      };
      if (acceptance?.paused && new Uint8Array(buffer)[0] === 0) {
        acceptance.pending.push(apply); acceptance.pendingBytes += buffer.byteLength - 9;
        acceptance.maxPendingBytes = Math.max(acceptance.maxPendingBytes, acceptance.pendingBytes);
      } else apply();
    };
    try {
      await invoke("desktop_terminal_end", { sessionId: sessionId.current });
      sessionId.current = null;
      mustEnd.current = false;
      const value = await invoke<Opened>("desktop_terminal_start", { execution: selection, cols: Math.max(20, Math.min(500, terminal.current.cols)), rows: Math.max(5, Math.min(200, terminal.current.rows)), events: channel });
      if (gen !== generation.current) return;
      sessionId.current = value.session_id;
      const acceptance = (window as unknown as { __PIXELS_TERMINAL_ACCEPTANCE__?: { sessionId?: string } }).__PIXELS_TERMINAL_ACCEPTANCE__;
      if (acceptance) acceptance.sessionId = value.session_id;
      mustEnd.current = true;
      setOpened(value); live.current = true; setPhase("running");
      setPrivacyHintDismissed(false);
      await invoke("desktop_terminal_watch", { sessionId: value.session_id });
      terminal.current.focus();
      localStorage.setItem(`pab.desktopTerminal.account.${code}`, selection.mode === "service" ? "service" : value.identity.account_id);
    } catch (cause) { setError(terminalError(cause)); live.current = false; setPhase("ended"); }
  }
  useEffect(() => {
    if (phase !== "running" || !opened) return;
    let active = true, pending = false;
    const sessionId = opened.session_id;
    const poll = async () => {
      if (pending) return; pending = true;
      try { const path = await invoke<string | null>("desktop_terminal_connection_path", {sessionId}); if (active) setOpened(current => current?.session_id === sessionId ? {...current, connection_path: path || "Unknown"} : current); }
      catch { if (active) setOpened(current => current?.session_id === sessionId ? {...current, connection_path:"Unknown"} : current); }
      finally { pending = false; }
    };
    const timer = window.setInterval(() => void poll(), 2000);
    return () => {active = false; window.clearInterval(timer);};
  }, [phase, opened?.session_id]);
  const options = [{ value: "service", label: text.service }, ...accounts.filter((entry) => entry.mode === "user").map((entry, index) => ({ value: entry.selection && "context_ref" in entry.selection ? entry.selection.context_ref : `unavailable-${index}`, label: `${entry.account_name}${entry.session_id ? ` · ${entry.session_id}` : ""}${entry.unavailable_reason ? ` · ${userFacingError(entry.unavailable_reason)}` : ""}`, disabled: !entry.selection }))];
  return <div className={`desktop-terminal ${dark ? "desktop-terminal-dark" : ""}`}>
    <header className="titlebar desktop-terminal-header"
      onMouseDown={(event) => {
        if (event.button !== 0 || event.detail !== 1 || (event.target as HTMLElement).closest("button, .desktop-terminal-actions, .window-controls")) return;
        event.preventDefault();
        void appWindow.startDragging().catch((cause) => setError(terminalError(cause)));
      }}
      onDoubleClick={(event) => {
        if ((event.target as HTMLElement).closest("button, .desktop-terminal-actions, .window-controls")) return;
        void appWindow.toggleMaximize().catch((cause) => setError(terminalError(cause)));
      }}>
      <div className="titlebar-identity desktop-terminal-title"><img className="brand-mark" src={brand} alt="" /><strong>{text.title}</strong><span>{code.replace(/(\d{3})(?=\d)/g, "$1 ")}</span><Tag color={phase === "running" ? "green" : "default"}>{phase === "preparing" ? text.preparing : phase === "starting" ? text.starting : phase === "running" ? text.running + (opened && ["P2P", "Relay"].includes(opened.connection_path) ? (language === "en" ? ` (${opened.connection_path})` : `（${opened.connection_path}）`) : "") : phase === "ended" ? text.ended : text.choose}</Tag>
        {launchMode === "system" && <span className="desktop-terminal-path">System</span>}

      </div>
      <Space className="desktop-terminal-actions" size={6}><Button size="small" aria-label={text.copy} title={text.copy} icon={<Copy size={14} />} onClick={() => void navigator.clipboard.writeText(terminal.current?.getSelection() || "").catch((cause) => setError(terminalError(cause)))} />
        <Button size="small" aria-label={text.paste} title={text.paste} disabled={phase !== "running"} icon={<Clipboard size={14} />} onClick={() => void navigator.clipboard.readText().then(paste).catch((cause) => setError(terminalError(cause)))} />
        <Button size="small" aria-label={text.clear} title={text.localClear} icon={<Eraser size={14} />} onClick={() => terminal.current?.clear()}><span className="desktop-terminal-action-label">{text.clear}</span></Button>
        <Button size="small" aria-label={text.close} title={text.close} danger disabled={phase !== "running"} icon={<Square size={13} />} onClick={() => void requestClose(false)}><span className="desktop-terminal-action-label">{text.close}</span></Button></Space>
      <div className="window-controls">
        <button aria-label={text.minimize} title={text.minimize} onClick={() => void appWindow.minimize().catch((cause) => setError(terminalError(cause)))}><Minus /></button>
        <button aria-label={text.maximize} title={text.maximize} onClick={() => void appWindow.toggleMaximize().catch((cause) => setError(terminalError(cause)))}><Maximize2 /></button>
        <button className="close" aria-label={text.closeWindow} title={text.closeWindow} onClick={() => void requestClose(true)}><X /></button>
      </div>
    </header>
    {!launchMode && (phase === "ready" || phase === "ended" || phase === "preparing") && <div className="desktop-terminal-setup"><Select aria-label={text.account} style={{ minWidth: 250, maxWidth: 520, flex: 1 }} placeholder={text.choose} value={selected || undefined} options={options} onChange={setSelected} showSearch optionFilterProp="label" loading={loadingAccounts} />
      <Button icon={<RefreshCw size={14} />} onClick={() => void refreshAccounts()} loading={loadingAccounts}>{text.refresh}</Button><Button type="primary" disabled={!selected || loadingAccounts} onClick={() => void start()}>{opened ? text.reopen : text.open}</Button></div>}
    {launchMode && phase === "ended" && <div className="desktop-terminal-setup"><Button type="primary" onClick={() => void automaticStart()}>{text.reopen}</Button></div>}
    {error && <Alert className="desktop-terminal-alert" type="error" showIcon title={text.error} description={userFacingError(error)} closable onClose={() => setError("")} />}
    {reason && <Alert className="desktop-terminal-alert" type="info" title={`${text.endedReason}: ${reason}`} />}
    {opened?.os_family === "macos" && phase === "running" && !privacyHintDismissed && <Alert className="desktop-terminal-alert" type="info" showIcon title={text.macosFilesTitle} description={text.macosFilesBody} closable onClose={() => setPrivacyHintDismissed(true)} />}
    <div className="desktop-terminal-screen"><div className="desktop-terminal-host" ref={host} /></div>
  </div>;
}
