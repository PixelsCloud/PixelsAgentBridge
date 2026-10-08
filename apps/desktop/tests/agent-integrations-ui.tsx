import { createRoot } from "react-dom/client";
import { App, ConfigProvider, theme } from "antd";
import { AgentIntegrationsPanel } from "../src/AgentIntegrationsPanel";
import { McpConnectionsPanel } from "../src/McpConnectionsPanel";
import DesktopApp from "../src/App";
import type { Language } from "../src/i18n";
import "../src/App.css";
import "../src/theme.css";
const params = new URLSearchParams(location.search);
const dark = params.get("theme") === "dark";
if (params.has("app")) {
  localStorage.setItem("pab.language", params.get("language") ?? "en");
  localStorage.setItem("pab.theme", dark ? "dark" : "light");
}
document.documentElement.dataset.theme = dark ? "dark" : "light";
const calls: unknown[] = [];
const states = Object.fromEntries(["codex", "kimi", "claude", "deepseek", "opencode"].map(id => [id, {
  id, detected: true, mcpAvailable: true, configured: id === "codex", enabled: id === "codex",
  conflict: false, configPath: `/fixture/${id}`, error: null as string | null,
}]));
if (params.get("error")) states.claude.error = "Invalid JSON configuration";
if (params.get("missing")) states.claude.detected = false;
if (params.get("repair")) states.codex.enabled = false;
const callbacks = new Map<number, (event: any) => void>();
const listeners = new Map<number, { event: string; handler: number }>();
let sequence = 0;
const account = { username: params.get("longName") ? "Pixels-very-long-account-name-for-layout" : "Pixels", userId: "user-123", serverAdmin: false, revision: 1 };
let activeScope = params.has("signedIn") ? account : null;
const fixture = (window as any).fixture = { calls, states, release: null as null | (() => void),
  loginError: params.has("loginError"), logoutError: params.has("logoutError"), scopeError: params.has("scopeError"),
  listeners, emit: (event: string, payload: unknown) => {
    for (const [id, listener] of listeners) if (listener.event === event) callbacks.get(listener.handler)?.({ id, event, payload });
  } };
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: (_event: string, id: number) => listeners.delete(id) };
(window as any).__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: "main" } },
  transformCallback: (callback: (event: any) => void) => { const id = ++sequence; callbacks.set(id, callback); return id; },
  unregisterCallback: (id: number) => callbacks.delete(id), invoke: async (command: string, args: any) => {
  calls.push({ command, args });
  if (command === "plugin:event|listen") { const id = ++sequence; listeners.set(id, args); return id; }
  if (command === "plugin:event|unlisten") { listeners.delete(args.eventId); return; }
  if (command.startsWith("plugin:window|")) return;
  if (command === "set_tray_language") return;
  if (command === "macos_permissions") return null;
  if (command === "operator_current_traffic_scope") { if (fixture.scopeError) throw Error("Scope unavailable"); return activeScope; }
  if (["operator_login_account", "operator_register_account"].includes(command)) {
    if (params.has("slowLogin")) await new Promise<void>(resolve => { fixture.release = resolve; });
    if (fixture.loginError) throw Error("Invalid credentials");
    activeScope = account;
    return account;
  }
  if (command === "operator_use_guest_scope") { if (fixture.logoutError) throw Error("Sign out failed"); activeScope = null; return; }
  if (command === "device_status") return { device_code: "123456789", device_id: "fixture", temporary_password: "fixture", executor_running: true, control_phase: "authenticated" };
  if (command === "get_operator_server_settings") return { controlUrl: "", relayUrl: "" };
  if (command === "operator_bootstrap") return { devices: [], tasks: [], operations: [], taskBefore: null, operationBeforeStartedAtUnixMs: null, operationBeforeId: null, hasMoreTasks: false, hasMoreOperations: false, totalCount: 0 };
  if (command === "operator_operations") return [];
  if (command === "mcp_reporting_status") return { revision: 1, running: true, count: 9, error: null,
    clients: ["kimi-code", "kimi-code", "claude-code", "codex_cli_rs", "dsh-mcp-client", "cursor-agent", "opencode", "Custom Agent", null].map((clientName, index) => ({
      report: { sessionId: `session-${index}`, processId: 1000 + index, version: "fixture", os: "windows", architecture: "x86_64",
        startedAtUnixMs: 1, updatedAtUnixMs: 1, clientName, clientVersion: "client-version", activeCalls: [], recentCalls: [], runtime: null },
    })) };
  if (command === "agent_integration_status") return { ...states[args.id] };
  if (command === "set_agent_integration") {
    if (params.get("slow") && args.id === "kimi") await new Promise<void>(resolve => { fixture.release = resolve; });
    if (params.get("fail")) throw Error("Fixture: config changed during update");
    states[args.id].configured = args.enabled;
    states[args.id].enabled = args.enabled;
    return { ...states[args.id] };
  }
  throw Error(`Unexpected command ${command}`);
} };
createRoot(document.getElementById("root")!).render(params.has("app") ? <DesktopApp /> : <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm }}><App>
  <div style={{ padding: 10, height: "100vh", overflow: "auto" }} className="settings-ai">
    {params.has("connections") ? <McpConnectionsPanel language={(params.get("language") ?? "en") as Language} />
      : <AgentIntegrationsPanel language={(params.get("language") ?? "en") as Language} />}
  </div>
</App></ConfigProvider>);
