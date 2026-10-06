import { useEffect, useRef, useState } from "react";
import { Alert, Button, Select, Space } from "antd";
import { RefreshCw } from "lucide-react";
import { messages, type Language } from "./i18n";
import { executionQuery, executionErrorMessage, type ExecutionEntry, type ExecutionSelection } from "./executionQueries";

export function useExecutionContexts(code: string | undefined, connected: boolean) {
  const [entries, setEntries] = useState<ExecutionEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [truncated, setTruncated] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const generation = useRef(0);
  useEffect(() => {
    const current = ++generation.current;
    setEntries([]); setError(""); setTruncated(false); setLoading(false);
    if (!code || !connected) return;
    setLoading(true);
    void executionQuery(code, { action: "execution_contexts", user: null, include_system: false, limit: 1000 }, crypto.randomUUID())
      .then((reply) => {
        if (generation.current !== current) return;
        if (reply.state !== "completed" || reply.data?.type !== "execution_contexts") throw new Error(reply.error || reply.state);
        setEntries(reply.data.entries); setTruncated(reply.truncated);
      })
      .catch((cause) => { if (generation.current === current) setError(executionErrorMessage(cause)); })
      .finally(() => { if (generation.current === current) setLoading(false); });
    return () => { generation.current++; };
  }, [code, connected, refresh]);
  return { entries, loading, error, truncated, refresh: () => setRefresh((v) => v + 1) };
}

export function selectedExecution(entries: ExecutionEntry[], key: string, mode: "user" | "desktop_user"): ExecutionSelection | null {
  if (mode === "user" && key === "service") return { mode: "service" };
  return entries.find((entry) => entry.mode === mode && entry.selection && "context_ref" in entry.selection && entry.selection.context_ref === key)?.selection ?? null;
}

export function ExecutionPicker({ language, contexts, mode, value, onChange, connected }: {
  language: Language;
  contexts: ReturnType<typeof useExecutionContexts>;
  mode: "user" | "desktop_user";
  value: string;
  onChange: (key: string) => void;
  connected: boolean;
}) {
  const t = messages[language];
  const entries = contexts.entries.filter((entry) => entry.mode === mode);
  const options = entries.map((entry, i) => ({
    value: entry.selection && "context_ref" in entry.selection ? entry.selection.context_ref : `unavailable-${i}`,
    label: `${entry.account_name}${entry.session_id ? ` · ${t.executionSession} ${entry.session_id}` : ""}${entry.unavailable_reason ? ` · ${entry.unavailable_reason}` : ""}`,
    disabled: !entry.selection,
  }));
  if (mode === "user") options.unshift({ value: "service", label: t.executionService, disabled: false });
  const stale = !!value && !selectedExecution(contexts.entries, value, mode);
  if (stale) options.unshift({ value, label: t.executionExpired, disabled: true });
  return <div className="execution-picker">
    <label className="field-label">{mode === "user" ? t.executionUser : t.executionDesktop}</label>
    <Space.Compact style={{ width: "100%" }}>
      <Select aria-label={mode === "user" ? t.executionUser : t.executionDesktop} showSearch optionFilterProp="label"
        style={{ flex: 1, minWidth: 0 }} value={value || undefined} placeholder={t.executionChoose}
        options={options} loading={contexts.loading} disabled={!connected} onChange={onChange} />
      <Button aria-label={t.executionRefresh} title={t.executionRefresh} disabled={!connected} loading={contexts.loading}
        icon={<RefreshCw size={15} />} onClick={contexts.refresh} />
    </Space.Compact>
    {contexts.error && <Alert type="warning" title={contexts.error} showIcon />}
    {stale && <small>{t.executionExpired}</small>}
    {contexts.truncated && <small>{t.executionTruncated}</small>}
    {mode === "desktop_user" && !contexts.loading && !entries.some((entry) => entry.selection) && <small>{t.executionNoDesktop}</small>}
  </div>;
}
