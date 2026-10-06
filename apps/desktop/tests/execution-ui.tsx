// Browser fixture uses the real panels. Native calls are recorded and answered
// with deterministic receipts; installed-device acceptance is separate.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { ConfigProvider, App, theme } from "antd";
import { RemoteOperationsPanel, type OperationKind } from "../src/RemoteOperationsPanel";
import type { Language } from "../src/i18n";
import "../src/App.css";
import "../src/theme.css";

const params = new URLSearchParams(location.search);
const language = (params.get("language") ?? "en") as Language;
const dark = params.get("theme") === "dark";
document.documentElement.dataset.theme = dark ? "dark" : "light";
const fixture = (window as any).fixture = { calls: [] as any[], revision: 0, pending: new Map(), delayTerminal: false, releaseTerminal: null };
const identity = (desktop = false) => ({ mode: desktop ? "desktop_user" : "user", account_name: "Alice", account_id: "uid:501", home: "/Users/alice", session_id: "2", logon_id: "native-fixture" });
const completed = (id: string, data: unknown) => ({ request_id: id, state: "completed", error: null, truncated: false, data });
const receipt = (id: string) => completed(id, { type: "applications", snapshot: { type: "action", result: { request_accepted: true, execution_identity: identity(true), notes: [], window_ready: null } } });
(window as any).__TAURI_INTERNALS__ = { invoke: async (command: string, args: any) => {
  fixture.calls.push({ command, args });
  if (command === "operator_execution_query") {
    if (fixture.rejectNext && args.query.action === "applications") { fixture.rejectNext = false; throw { message: "fixture request rejected before submission", outcome: "not_submitted", request_id: args.requestId }; }
    if (args.query.action === "execution_contexts") {
      const revision = ++fixture.revision;
      return completed(args.requestId, { type: "execution_contexts", entries: ["user", "desktop_user"].map((mode) => ({ mode, account_name: "Alice", session_id: "2", account_id: "uid:501", identity: identity(mode === "desktop_user"), unavailable_reason: null, selection: { mode, context_ref: `${mode}-${revision}` } })) });
    }
    if (args.query.query.action === "list") return completed(args.requestId, { type: "applications", snapshot: { type: "list", snapshot: { apps: [{ name: "Fixture Editor", app_id: "fixture.editor", path: "/Applications/Fixture Editor.app", source: "fixture", instance: null }], truncated: false, warnings: [], execution_identity: identity(true) } } });
    fixture.pending.set(args.requestId, receipt(args.requestId));
    return { request_id: args.requestId, state: "unconfirmed", error: null, data: null, truncated: false };
  }
  if (command === "operator_execution_query_result") return fixture.pending.get(args.requestId);
  if (command === "operator_open_terminal") {
    const reply = { sessionId: crypto.randomUUID(), shell: "fixture-shell", cols: 80, rows: 24, executionIdentity: identity() };
    fixture.terminalReply = reply;
    if (fixture.delayTerminal) await new Promise((resolve) => { fixture.releaseTerminal = resolve; });
    return reply;
  }
  if (command === "operator_terminal_read") return { data: "", ended: false };
  if (command === "operator_terminal_close") return;
  throw Error(`unexpected fixture call: ${command}`);
} };

function Panel() {
  const [operation, setOperation] = useState<OperationKind>("command");
  const [program, setProgram] = useState("fixture-program");
  const [code, setCode] = useState("123456789");
  fixture.switchDevice = () => setCode("987654321");
  return <ConfigProvider theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm, token: {
    colorPrimary: dark ? "#93d7c2" : "#218574", colorBgContainer: dark ? "#2b3038" : "#ffffff", colorBgElevated: dark ? "#333a43" : "#ffffff", colorBorder: dark ? "#434b55" : "#d4e0e5", colorText: dark ? "#f0f3f5" : "#162637", borderRadius: 9, fontSize: 12, controlHeight: 38,
  } }}><App><main className="surface" style={{ height: "94vh", margin: 10, padding: 20, overflow: "auto" }}>
    <RemoteOperationsPanel key={code} language={language} selected={{ deviceId: "fixture", deviceCode: code, alias: "Test Mac", osFamily: "macos", osReminder: "fixture", connected: true }}
      operation={operation} onOperationChange={setOperation} program={program} onProgramChange={setProgram} argumentsText="" onArgumentsTextChange={() => {}} cwd="" onCwdChange={() => {}}
      submitting={false} onRunCommand={(execution) => fixture.calls.push({ command: "ui_run_command", args: { execution } })}
      transferDirection="upload" onTransferDirectionChange={() => {}} transferSource="" onTransferSourceChange={() => {}} transferDestination="" onTransferDestinationChange={() => {}}
      transferOverwrite={false} onTransferOverwriteChange={() => {}} transfer={null} startingTransfer={false} onStartTransfer={() => {}} onCancelTransfer={() => {}} onAuditChange={() => {}} />
  </main></App></ConfigProvider>;
}
createRoot(document.getElementById("root")!).render(<Panel />);
