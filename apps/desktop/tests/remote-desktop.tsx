import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
localStorage.setItem("pab.language", new URLSearchParams(location.search).get("language") || "en");
localStorage.setItem("pab.theme", new URLSearchParams(location.search).get("theme") || "light");
const callbacks = new Map<number, (value: unknown) => void>();
const listeners = new Map<string, number>();
let next = 1;
const screens = [
  { id: "adapter:primary", name: "Main", desktop_width: 1920, desktop_height: 1080, primary: true },
  { id: "adapter:left", name: "Portrait", desktop_width: 1080, desktop_height: 1920, primary: false },
];
let opened = { config: { session_id: "viewer-session-fixture", screen: screens[0], fps: 15, generation: 1 }, screens, input_supported: true };
let phase = "viewing";
const params = new URLSearchParams(location.search);
let inputReady = params.get("ready") !== "0", inputEnabled = inputReady, inputRequested = true;
let fullscreen = false;
const remoteOs = params.get("os") || "windows";
let decoder: string | null = null;
let clipboardEnabled = true, clipboardState = "active";
const status = () => ({ phase, opened, input_ready: inputReady, input_enabled: inputEnabled, frame_ready: inputReady, decoder, remote_os: remoteOs, connection_path: "p2p", clipboard_enabled: clipboardEnabled, clipboard_state: clipboardState });
const fixture = (window as any).fixture = {
  calls: [] as any[], reject: false, rejectFullscreen: false,
  holdFullscreen: false, releaseFullscreen: undefined as (() => void) | undefined,
  closeAnswers: [] as boolean[], closeDialogs: 0, closed: false, rejectClose: false,
  holdClose: false, releaseClose: undefined as (() => void) | undefined,
  assistance: [] as any[],
  holdCompletion: [] as string[],
  deferredCompletion: {} as Record<string, { resolve: () => void; reject: (message: string) => void }>,
  decoder(value: string | null) { decoder = value; fixture.emit(status()); },
  performance(value: unknown) { callbacks.get(listeners.get("desktop-viewer-performance")!)?.({ payload: value }); },
  clipboard(value: string) { clipboardState = value; fixture.emit(status()); },
  emit(value: unknown) { callbacks.get(listeners.get("desktop-viewer-status")!)?.({ event: "desktop-viewer-status", id: 1, payload: value }); },
  fail() { phase = "failed"; fixture.emit({ phase, error: "Capture helper unavailable" }); },
  recover() { phase = "reconnecting"; inputEnabled = false; inputReady = false; decoder = null; fixture.emit(status()); },
  ready(value: boolean, supported = true) { inputReady = value; opened.input_supported = supported; inputEnabled = value && supported && inputRequested; fixture.emit(status()); },
  fullscreen(value: boolean) { fullscreen = value; callbacks.get(listeners.get("desktop-viewer-fullscreen")!)?.({ payload: value }); },
  nativeEscape() { fixture.fullscreen(false); },
  nativeClose() { callbacks.get(listeners.get("desktop-viewer-close-requested")!)?.({ payload: null }); },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "desktop-viewer-fixture" }, currentWebview: { label: "desktop-viewer-fixture" } },
  transformCallback(callback: (value: unknown) => void) { const id = next++; callbacks.set(id, callback); return id; },
  unregisterCallback(id: number) { callbacks.delete(id); },
  invoke: async (command: string, args: any) => {
    fixture.calls.push({ command, args });
    if (command === "plugin:event|listen") { listeners.set(args.event, args.handler); return 1; }
    if (command === "plugin:window|is_fullscreen") return fullscreen;
    if (command === "plugin:window|title") return "Pixels · fixture remote computer";
    if (command.startsWith("plugin:")) return null;
    if (command === "desktop_viewer_fullscreen") {
      if (fixture.rejectFullscreen) throw "Fullscreen unavailable";
      fixture.fullscreen(args.fullscreen);
      const reply = fullscreen;
      if (fixture.holdFullscreen) await new Promise<void>(resolve => { fixture.releaseFullscreen = resolve; });
      return reply;
    }
    if (command === "desktop_viewer_start") return status();
    if (command === "desktop_viewer_close") {
      fixture.closeDialogs++;
      if (fixture.holdClose) await new Promise<void>(resolve => { fixture.releaseClose = resolve; });
      if (fixture.rejectClose) throw "Could not show close confirmation";
      const confirmed = fixture.closeAnswers.shift() ?? false;
      if (confirmed) fixture.closed = true;
      return confirmed;
    }
    if (command === "desktop_viewer_overlay") return null;
    if (command === "desktop_viewer_assistance") return fixture.assistance;
    if (command === "desktop_viewer_assistance_complete") {
      if (fixture.holdCompletion.includes(args.id)) {
        await new Promise<void>((resolve, reject) => { fixture.deferredCompletion[args.id] = { resolve, reject }; });
      }
      fixture.assistance = fixture.assistance.filter((row: any) => row.assistance_id !== args.id);
      return null;
    }
    if (command === "desktop_viewer_control") {
      if (fixture.reject) throw "Renderer is busy; retry shortly";
      if (typeof args.clipboardEnabled === "boolean") { clipboardEnabled = args.clipboardEnabled; clipboardState = clipboardEnabled ? "active" : "disabled"; fixture.emit(status()); return null; }
      fixture.emit({ phase: "switching", opened });
      if (typeof args.inputEnabled === "boolean") inputRequested = args.inputEnabled;
      inputEnabled = false; inputReady = false;
      if (args.screen) opened = { ...opened, config: { ...opened.config, screen: screens.find((s) => s.id === args.screen)!, generation: opened.config.generation + 1 } };
      if (args.fps) opened = { ...opened, config: { ...opened.config, fps: args.fps } };
      setTimeout(() => { phase = "viewing"; inputReady = true; inputEnabled = inputRequested && opened.input_supported; fixture.emit(status()); }, 40);
      return null;
    }
    throw new Error(`Unexpected viewer call: ${command}`);
  },
};
const { default: RemoteDesktopWindow } = await import("../src/RemoteDesktopWindow");
createRoot(document.getElementById("root")!).render(<StrictMode><RemoteDesktopWindow /></StrictMode>);
