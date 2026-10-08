import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Space, Tag } from "antd";
import { RotateCw } from "lucide-react";
import { messages, type Language } from "./i18n";

const clients = [
  { id: "codex", name: "Codex" }, { id: "kimi", name: "Kimi Code" },
  { id: "claude", name: "Claude Code" }, { id: "deepseek", name: "DeepSeek Harness" },
  { id: "opencode", name: "OpenCode" },
] as const;
type AgentId = typeof clients[number]["id"];
type Integration = {
  id: AgentId; detected: boolean; mcpAvailable: boolean; configured: boolean;
  enabled: boolean; conflict: boolean; configPath: string; error: string | null;
};

function AgentRow({ id, name, language }: { id: AgentId; name: string; language: Language }) {
  const t = messages[language];
  const [state, setState] = useState<Integration | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const revision = useRef(0);
  async function refresh() {
    const current = ++revision.current;
    setBusy(true);
    setError("");
    try {
      const result = await invoke<Integration>("agent_integration_status", { id });
      if (current === revision.current) setState(result);
    } catch (reason) { if (current === revision.current) setError(String(reason)); }
    finally { if (current === revision.current) setBusy(false); }
  }
  useEffect(() => { void refresh(); return () => { revision.current++; }; }, [id]);
  async function change(enabled: boolean) {
    const current = ++revision.current;
    setBusy(true); setError("");
    try {
      const result = await invoke<Integration>("set_agent_integration", { id, enabled });
      if (current === revision.current) setState(result);
    } catch (reason) {
      if (current === revision.current) {
        setError(String(reason));
        // Re-read after a partial/failed update instead of retaining a stale success.
        try {
          const result = await invoke<Integration>("agent_integration_status", { id });
          if (current === revision.current) setState(result);
        } catch { /* Retain the original operation error. */ }
      }
    } finally { if (current === revision.current) setBusy(false); }
  }
  const failure = error || state?.error || "";
  const status = failure ? t.settingsAiError : !state ? t.loading
    : state.conflict ? t.settingsAiConflict : !state.detected ? t.settingsAiUnavailable
      : !state.mcpAvailable ? t.settingsAiMissingMcp : state.enabled ? t.settingsAiEnabled
        : state.configured ? t.settingsAiNeedsRepair : t.settingsAiDisabled;
  return <div className="settings-agent" data-testid={`agent-${id}`}>
    <div className="settings-agent-row">
      <div><strong>{name}</strong><Tag color={failure || state?.conflict ? "error" : state?.enabled ? "success" : "default"}>{status}</Tag></div>
      <Space wrap>
        {state?.configured && !state.enabled && !state.conflict && <Button disabled={busy || !!state.error} onClick={() => void change(false)}>{t.settingsAiDisable}</Button>}
        <Button type={state?.enabled ? "default" : "primary"} loading={busy}
          aria-label={state?.enabled ? t.settingsAiDisable : state?.configured ? t.settingsAiRepair : t.settingsAiEnable}
          disabled={busy || !state || state.conflict || !!state.error || (!state.enabled && (!state.detected || !state.mcpAvailable))}
          onClick={() => void change(!state?.enabled)}>
          {state?.enabled ? t.settingsAiDisable : state?.configured ? t.settingsAiRepair : t.settingsAiEnable}
        </Button>
        <Button type="text" disabled={busy} aria-label={`${t.settingsAiRefresh} ${name}`} title={t.settingsAiRefresh}
          icon={<RotateCw size={15} />} onClick={() => void refresh()} />
      </Space>
    </div>
    {failure && <Alert role="alert" type="error" showIcon title={failure} />}
  </div>;
}

export function AgentIntegrationsPanel({ language }: { language: Language }) {
  return <div className="settings-agent-list">{clients.map(client => <AgentRow key={client.id} {...client} language={language} />)}</div>;
}
