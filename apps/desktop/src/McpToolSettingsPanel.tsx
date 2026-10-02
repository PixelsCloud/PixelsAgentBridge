import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, App, Button, Spin, Switch, Tag } from "antd";
import { messages, type Language } from "./i18n";

type ToolGroup = "core" | "file" | "system" | "desktop" | "git" | "container";
type ToolSettings = { version: number; enabledGroups: ToolGroup[] };
type ToolSettingsView = {
  settings: ToolSettings;
  loadError: string | null;
  groups: { group: ToolGroup; toolCount: number; required: boolean }[];
};

export function McpToolSettingsPanel({ language }: { language: Language }) {
  const t = messages[language];
  const { message } = App.useApp();
  const [view, setView] = useState<ToolSettingsView | null>(null);
  const [savedGroups, setSavedGroups] = useState<ToolGroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  async function load() {
    setLoading(true); setError("");
    try {
      const value = await invoke<ToolSettingsView>("get_mcp_tool_settings");
      setView(value); setSavedGroups(value.loadError ? [] : value.settings.enabledGroups);
      setError(value.loadError ? `${t.settingsToolsReadError} ${value.loadError}` : "");
    } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }
  useEffect(() => { void load(); }, []);
  const changed = !!view && (savedGroups.length !== view.settings.enabledGroups.length || savedGroups.some(group => !view.settings.enabledGroups.includes(group)));
  const count = view?.groups.filter(item => view.settings.enabledGroups.includes(item.group)).reduce((sum, item) => sum + item.toolCount, 0) ?? 0;
  async function save() {
    if (!view) return;
    setSaving(true); setError("");
    const settings = view.settings;
    try {
      await invoke("save_mcp_tool_settings", { settings });
      setSavedGroups(settings.enabledGroups);
      void message.success(t.settingsToolsSaved);
    } catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  }
  return <div className="settings-tools">
    <h2>{t.settingsTools}</h2>
    <p>{t.settingsToolsHint}</p>
    {loading && <Spin />}
    {error && <Alert type="error" showIcon title={error} action={!view && <Button size="small" onClick={() => void load()}>{t.settingsToolsRetry}</Button>} />}
    {view && <>
      <div className="settings-tool-groups">
        {view.groups.map(({ group, toolCount, required }) => <div className="settings-tool-group" key={group}>
          <div>
            <label htmlFor={`mcp-group-${group}`}><strong>{t.toolGroups[group].name}</strong> <Tag>{toolCount}</Tag>{required && <Tag>{t.settingsToolsRequired}</Tag>}</label>
            <p>{t.toolGroups[group].description}</p>
          </div>
          <Switch id={`mcp-group-${group}`} aria-label={t.toolGroups[group].name} checked={view.settings.enabledGroups.includes(group)} disabled={required || saving || loading}
            onChange={checked => setView({ ...view, settings: { ...view.settings, enabledGroups: checked ? [...view.settings.enabledGroups, group] : view.settings.enabledGroups.filter(item => item !== group) } })} />
        </div>)}
      </div>
      <div className="settings-tools-actions">
        <span>{t.settingsToolsCount.replace("{count}", String(count))}</span>
        <Button type="primary" disabled={!changed || loading} loading={saving} onClick={() => void save()}>{t.settingsToolsSave}</Button>
      </div>
    </>}
  </div>;
}
