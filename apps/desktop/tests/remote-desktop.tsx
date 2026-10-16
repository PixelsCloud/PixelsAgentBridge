import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
localStorage.setItem("pab.language", "en");
const callbacks = new Map<number, (value: unknown) => void>();
const listeners = new Map<string, number>();
let next = 1;
const screens = [
  { id: "adapter:primary", name: "Main", desktop_width: 1920, desktop_height: 1080, primary: true },
  { id: "adapter:left", name: "Portrait", desktop_width: 1080, desktop_height: 1920, primary: false },
];
let opened = { config: { screen: screens[0], fps: 15, generation: 1 }, screens };
let phase = "viewing";
const fixture = (window as any).fixture = {
  calls: [] as any[], reject: false,
  emit(value: unknown) { callbacks.get(listeners.get("desktop-viewer-status")!)?.({ event: "desktop-viewer-status", id: 1, payload: value }); },
  fail() { phase = "failed"; fixture.emit({ phase, error: "Capture helper unavailable" }); },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "desktop-viewer-fixture" }, currentWebview: { label: "desktop-viewer-fixture" } },
  transformCallback(callback: (value: unknown) => void) { const id = next++; callbacks.set(id, callback); return id; },
  unregisterCallback(id: number) { callbacks.delete(id); },
  invoke: async (command: string, args: any) => {
    fixture.calls.push({ command, args });
    if (command === "plugin:event|listen") { listeners.set(args.event, args.handler); return 1; }
    if (command.startsWith("plugin:")) return null;
    if (command === "desktop_viewer_start") return { phase, opened };
    if (command === "desktop_viewer_control") {
      if (fixture.reject) throw "Renderer is busy; retry shortly";
      fixture.emit({ phase: "switching", opened });
      if (args.screen) opened = { ...opened, config: { ...opened.config, screen: screens.find((s) => s.id === args.screen)!, generation: opened.config.generation + 1 } };
      if (args.fps) opened = { ...opened, config: { ...opened.config, fps: args.fps } };
      setTimeout(() => { phase = "viewing"; fixture.emit({ phase, opened }); }, 40);
      return null;
    }
    throw new Error(`Unexpected viewer call: ${command}`);
  },
};
const { default: RemoteDesktopWindow } = await import("../src/RemoteDesktopWindow");
createRoot(document.getElementById("root")!).render(<StrictMode><RemoteDesktopWindow /></StrictMode>);
