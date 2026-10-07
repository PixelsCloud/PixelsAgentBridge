import { useEffect, useRef, useState } from "react";
import { Alert, Button, Checkbox, Input, Segmented, Space, Table } from "antd";
import { messages, type Language } from "./i18n";
import { ExecutionPicker, selectedExecution, type useExecutionContexts } from "./ExecutionPicker";
import { ExecutionIdentityView } from "./ExecutionIdentityView";
import { executionQuery, executionErrorMessage, observeExecutionQuery, isPending, type AppInfo, type AppTarget, type ExecutionReply } from "./executionQueries";

export function ApplicationBrowser({ code, connected, language, contexts, osFamily, onAuditChange }: {
  code: string; connected: boolean; language: Language;
  contexts: ReturnType<typeof useExecutionContexts>; onAuditChange: () => void;
  osFamily?: string;
}) {
  const t = messages[language];
  const [desktop, setDesktop] = useState("");
  const [scope, setScope] = useState<"installed" | "running">("installed");
  const [search, setSearch] = useState("");
  const [apps, setApps] = useState<AppInfo[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [file, setFile] = useState("");
  const [withSelected, setWithSelected] = useState(false);
  const [newInstance, setNewInstance] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [reply, setReply] = useState<ExecutionReply | null>(null);
  const [unresolved, setUnresolved] = useState<string | null>(null);
  const active = useRef(true);
  useEffect(() => { active.current = true; return () => { active.current = false; }; }, []);
  // Parent keys this panel by device/account. Unmounting never cancels a
  // dispatched mutation or changes the original request ID.
  const execution = selectedExecution(contexts.entries, desktop, "desktop_user");
  const chosen = selected === null ? undefined : apps[selected];
  const target: AppTarget | null = chosen?.app_id ? { kind: "id", id: chosen.app_id } : chosen?.path ? { kind: "path", path: chosen.path } : null;
  const unavailable = !connected || !execution || busy || !!unresolved;
  function accept(next: ExecutionReply) {
    if (!active.current) return;
    setReply(next); setUnresolved(isPending(next) ? next.request_id : null);
    if (next.data?.type === "applications" && next.data.snapshot.type === "list") {
      setApps(next.data.snapshot.snapshot.apps); setSelected(null);
    }
  }
  async function run(query: unknown) {
    if (unavailable) return;
    const id = crypto.randomUUID();
    setBusy(true); setError(""); setReply(null); setUnresolved(id);
    try {
      const next = await executionQuery(code, { action: "applications", execution, query }, id, accept);
      if (active.current) accept(next);
    } catch (cause) {
      if (active.current) {
        setError(executionErrorMessage(cause));
        if (typeof cause === "object" && cause !== null && "outcome" in cause && cause.outcome === "not_submitted") setUnresolved(null);
      }
    }
    finally { if (active.current) setBusy(false); onAuditChange(); }
  }
  async function inspect() {
    if (!unresolved || busy) return;
    setBusy(true); setError("");
    try { accept(await observeExecutionQuery(code, unresolved)); }
    catch (cause) { if (active.current) setError(executionErrorMessage(cause)); }
    finally { if (active.current) setBusy(false); onAuditChange(); }
  }
  const snapshot = reply?.data?.type === "applications" ? reply.data.snapshot : null;
  const identity = snapshot?.type === "list" ? snapshot.snapshot.execution_identity : snapshot?.type === "action" ? snapshot.result.execution_identity : reply?.execution_context?.identity;
  return <div className="application-browser">
    <ExecutionPicker language={language} contexts={contexts} mode="desktop_user" value={desktop} connected={connected && !busy && !unresolved}
      onChange={(value) => { setDesktop(value); setApps([]); setSelected(null); }} />
    <Segmented disabled={busy || !!unresolved} value={scope} options={[{ value: "installed", label: t.appsInstalled }, { value: "running", label: t.appsRunning }]}
      onChange={(value) => { setScope(value as typeof scope); setApps([]); setSelected(null); }} />
    <Space.Compact style={{ width: "100%" }}>
      <Input aria-label={t.appsSearch} placeholder={t.appsSearch} value={search} onChange={(event) => setSearch(event.target.value)} />
      <Button disabled={unavailable} loading={busy} onClick={() => void run({ action: "list", request: { scope, search, limit: 200 } })}>{t.appsRefresh}</Button>
    </Space.Compact>
    <Table size="small" pagination={false} scroll={{ y: 240 }} rowKey="key" dataSource={apps.map((app, key) => ({ ...app, key }))}
      rowSelection={{ type: "radio", selectedRowKeys: selected === null ? [] : [selected], onChange: (keys) => setSelected(Number(keys[0])) }}
      columns={[{ title: t.appsName, dataIndex: "name" }, { title: "PID", render: (_, app) => app.instance?.process_id ?? "—", width: 80 }]}
      locale={{ emptyText: t.appsEmpty }} />
    {osFamily === "macos" && <Checkbox checked={newInstance} disabled={busy || !!unresolved}
      onChange={event => setNewInstance(event.target.checked)}>{t.appsNewInstance}</Checkbox>}
    <Button type="primary" disabled={unavailable || !target} onClick={() => void run({ action: "execute", request: {
      operation: "launch", application: target, ...(osFamily === "macos" && newInstance ? { new_instance: true } : {}),
    } })}>{t.appsLaunch}</Button>
    <label className="field-label">{t.appsFilePath}</label>
    <Input value={file} onChange={(event) => setFile(event.target.value)} placeholder={t.appsFilePath} />
    <Checkbox checked={withSelected} disabled={!target} onChange={(event) => setWithSelected(event.target.checked)}>{t.appsUseSelected}</Checkbox>
    <Button disabled={unavailable || !file.trim() || (withSelected && !target)} onClick={() => void run({ action: "execute", request: { operation: "open_file", path: file.trim(), application: withSelected ? target : null } })}>{t.appsOpenFile}</Button>
    {error && <Alert type="error" title={error} showIcon />}
    {reply?.error && <Alert type="error" title={reply.error} showIcon />}
    {snapshot?.type === "action" && reply?.state === "completed" && <Alert type="success" title={t.appsAccepted} description={t.appsWindowNotConfirmed} showIcon />}
    {snapshot?.type === "list" && snapshot.snapshot.truncated && <Alert type="info" title={t.appsTruncated} />}
    {snapshot?.type === "list" && snapshot.snapshot.warnings.map((warning) => <Alert key={warning} type="warning" title={warning} />)}
    {unresolved && <Alert type={busy ? "info" : "warning"} title={busy ? t.appsWorking : t.appsUnconfirmed} description={<Button loading={busy} onClick={() => void inspect()}>{t.appsInspect}</Button>} />}
    {identity && <ExecutionIdentityView identity={identity} language={language} />}
  </div>;
}
