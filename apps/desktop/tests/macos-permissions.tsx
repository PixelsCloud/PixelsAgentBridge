import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { App, ConfigProvider, theme } from "antd";
import { SettingsPanel, type SettingsSection } from "../src/SettingsPanel";
import type { Language } from "../src/i18n";
import "../src/App.css";
import "../src/theme.css";
const params = new URLSearchParams(location.search);
const dark = params.get("theme") === "dark";
document.documentElement.dataset.theme = dark ? "dark" : "light";
const fixture = (window as any).fixture = {
  calls: [] as { command: string; args: any }[],
  clipboard: "", fail: "", accept: false,
  permissions: { screenRecording: params.has("allowed"), accessibility: params.has("allowed") },
  disk: Object.fromEntries(["desktop", "executor"].map(target => [target, { state: params.has("disk-allowed") ? "normal" : "restricted", folders: ["Downloads", "Documents", "Desktop"].map(folder => ({ folder, state: params.has("disk-allowed") ? "normal" : "restricted", reason: null })), source: "directory_enumeration", checkedAtUnixMs: Date.now() }])),
  holdDisk: "", resolveDisk: null as null | (() => void),
};
Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (value: string) => { if (fixture.fail === "clipboard") throw Error("Clipboard unavailable"); fixture.clipboard = value; } } });
(window as any).__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: "main" } }, invoke: async (command: string, args: any) => {
  fixture.calls.push({ command, args });
  if (fixture.fail === command) throw Error("Native action failed: " + command);
  if (command === "macos_permissions") return params.has("windows") ? null : { ...fixture.permissions };
  if (command === "get_operator_server_settings") return { controlUrl: "", relayUrl: "" };
  if (command === "macos_file_access_check") {
    if (fixture.holdDisk === args.target) await new Promise<void>(resolve => { fixture.resolveDisk = resolve; });
    if (args.target === "executor" && params.has("missing-service")) throw Error("Service missing");
    return structuredClone(fixture.disk[args.target]);
  }
  if (command === "macos_permission_targets") return [{ target: "desktop", path: "/Applications/Pixels Agent Bridge.app", exists: true }, { target: "executor", path: "/Library/Application Support/PixelsAgentBridge/pab-executor", exists: !params.has("missing-service") }];
  if (command === "request_macos_permission") {
    if (fixture.accept) fixture.permissions[args.permission === "screen" ? "screenRecording" : "accessibility"] = true;
    return { ...fixture.permissions };
  }
  if (["open_macos_permission_settings", "reveal_macos_permission_target", "restart_macos_permission_processes", "restart_macos_executor_for_permissions"].includes(command)) return;
  throw Error("Unexpected command: " + command);
} };
function Fixture() {
  const [section, setSection] = useState<SettingsSection>(params.has("windows") ? "preferences" : "permissions");
  const [language, setLanguage] = useState<Language>((params.get("language") || "en") as Language);
  return <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, token: { colorPrimary: "#218574", fontSize: 13 } }}><App>
    <div className="settings-layout" style={{ display: "grid", gridTemplateColumns: "180px minmax(0,1fr)", gap: 14, height: "100vh", boxSizing: "border-box", padding: 18 }}>
      <SettingsPanel language={language} section={section} onSectionChange={setSection} onLanguageChange={setLanguage} />
    </div>
  </App></ConfigProvider>;
}
createRoot(document.getElementById("root")!).render(<StrictMode><Fixture /></StrictMode>);
