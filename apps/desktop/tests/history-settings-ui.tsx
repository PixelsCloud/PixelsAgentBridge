// Real panels with deterministic native settings/history for browser regressions.
// This fixture does not replace installed-host acceptance.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { App, ConfigProvider, theme } from "antd";
import { SettingsPanel } from "../src/SettingsPanel";
import { ActivityPanel } from "../src/ActivityPanel";
import type { Language } from "../src/i18n";
import type { TaskEntry } from "../src/operatorTypes";
import "../src/App.css";
import "../src/theme.css";

const params = new URLSearchParams(location.search);
const dark = params.get("theme") === "dark";
document.documentElement.dataset.theme = dark ? "dark" : "light";
(window as any).__TAURI_INTERNALS__ = { invoke: async (command: string) => {
  if (command === "operator_server_settings") return { controlUrl: "", relayUrl: "" };
  if (command === "codex_integration_status") return { available: true, enabled: true, occupied: false };
  throw Error(`unexpected fixture call: ${command}`);
} };
const task: TaskEntry = {
  id: "long-title", deviceCode: "565893930", initiatedBy: "guest",
  program: "/a/very/long/path/".repeat(20) + "中文程序", args: [], cwd: null,
  startedAtUnixMs: 1791331200000, state: "Succeeded", complete: true,
  stdout: "fixture output", stderr: "", stdoutOffset: 0, stderrOffset: 0,
};
function Fixture() {
  const [language, setLanguage] = useState<Language>((params.get("language") ?? "en") as Language);
  return <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm }}><App>
    {params.get("panel") === "settings" ? <div className="settings-layout">
      <SettingsPanel language={language} section="preferences" onSectionChange={() => {}}
        onLanguageChange={value => { localStorage.setItem("pab.language", value); setLanguage(value); }} />
    </div> : <div style={{ width: 767, height: 650 }}>
      <ActivityPanel embedded={params.get("embedded") === "true"} language={language}
        tasks={[task]} operations={[]} totalCount={1} selectedId={task.id} onSelect={() => {}}
        hasMore={false} hasMoreTasks={false} hasMoreOperations={false} loadingMore={false}
        refreshing={false} refreshError="" onLoadMore={async () => false} onRefresh={() => {}} onRefreshTask={() => {}} />
    </div>}
  </App></ConfigProvider>;
}
createRoot(document.getElementById("root")!).render(<Fixture />);
