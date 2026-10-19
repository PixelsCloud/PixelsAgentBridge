import { createRoot } from "react-dom/client";
import { StrictMode } from "react";
localStorage.setItem("pab.language", new URLSearchParams(location.search).get("language") || "en");
localStorage.setItem("pab.theme", new URLSearchParams(location.search).get("theme") || "light");
const callbacks = new Map<number, (value: unknown) => void>();
const listeners = new Map<string, number>();
let next = 1;
let channel: any;
let index = 0;
let offset = 0;
const fixture = (window as any).fixture = {
  calls: [] as any[], failInput: false, failClose: false, failSelection: new URLSearchParams(location.search).has("no-console"), delayConsume: false, releaseConsume: null as (() => void) | null,
  output(text: string) { const bytes = new TextEncoder().encode(text); const frame = new Uint8Array(bytes.length + 9); new DataView(frame.buffer).setBigUint64(1, BigInt(offset)); frame.set(bytes, 9); channel.onmessage(frame.buffer); offset += bytes.length; },
  end() { const data = new TextEncoder().encode(JSON.stringify({ exit_code: 7, reason: "Shell exited" })); const frame = new Uint8Array(data.length + 9); frame[0] = 1; new DataView(frame.buffer).setBigUint64(1, BigInt(offset)); frame.set(data, 9); channel.onmessage(frame.buffer); },
  closeWindow() { callbacks.get(listeners.get("tauri://close-requested")!)?.({ event: "tauri://close-requested", id: 1, payload: null }); },
  paste(text: string) { const data = new DataTransfer(); data.setData("text/plain", text); document.querySelector(".desktop-terminal-host")!.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true })); },
  raw(bytes: number[]) { channel.onmessage(new Uint8Array(bytes).buffer); },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "desktop-terminal-fixture" }, currentWebview: { label: "desktop-terminal-fixture" } },
  transformCallback(callback: (value: unknown) => void) { const id = next++; callbacks.set(id, callback); return id; },
  unregisterCallback(id: number) { callbacks.delete(id); },
  invoke: async (command: string, args: any) => {
    fixture.calls.push({ command, args: command === "desktop_terminal_start" ? { ...args, events: undefined } : args });
    if (command === "plugin:event|listen") { listeners.set(args.event, args.handler); return 1; }
    if (command.startsWith("plugin:")) return null;
    if (command === "desktop_terminal_info") return { code: "428358431", launch_mode: new URLSearchParams(location.search).get("launch") };
    if (command === "desktop_terminal_selection") {
      if (fixture.failSelection) throw "no active local console login was found";
      return new URLSearchParams(location.search).get("launch") === "system" ? { mode: "service" } : { mode: "user", context_ref: "fixture-console" };
    }
    if (command === "desktop_terminal_accounts") return { state: "completed", truncated: false, data: { type: "execution_contexts", entries: [{ mode: "user", account_name: "Administrator", account_id: "sid:500", session_id: "2", selection: { mode: "user", context_ref: "fixture-user" }, unavailable_reason: null }] } };
    if (command === "desktop_terminal_start") { channel = args.events; index = 0; offset = 0; return { session_id: crypto.randomUUID(), shell: "powershell.exe", arguments: ["-NoLogo", "-NoExit"], cwd: "C:\\Users\\Administrator", identity: { mode: "user", account_name: "Administrator", account_id: "sid:500" }, cols: args.cols, rows: args.rows, connection_path: "P2P", os_family: new URLSearchParams(location.search).get("os") || "windows" }; }
    if (command === "desktop_terminal_consume" && fixture.delayConsume) { fixture.delayConsume = false; await new Promise<void>((resolve) => { fixture.releaseConsume = resolve; }); return null; }
    if (command === "desktop_terminal_input" && fixture.failInput) throw "input result unconfirmed; not replayed";
    if (command === "desktop_terminal_end" && fixture.failClose) throw "worker termination not confirmed";
    if (command.startsWith("desktop_terminal_")) return null;
    throw new Error(`unexpected terminal call: ${command} ${index}`);
  },
};
const { default: DesktopTerminalWindow } = await import("../src/DesktopTerminalWindow");
createRoot(document.getElementById("root")!).render(<StrictMode><DesktopTerminalWindow /></StrictMode>);
