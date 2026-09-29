import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowRight, Bot, Info, Languages, RotateCw, Server } from "lucide-react";
import { messages, type Language } from "./i18n";

type ServerSettings = {
  deploymentId: string;
  controlUrl: string;
  relayUrl: string;
};

type CodexIntegration = {
  available: boolean;
  enabled: boolean;
  occupied: boolean;
};

type Props = {
  language: Language;
  onLanguageChange: (value: Language) => void;
};

type SettingsSection = "preferences" | "ai" | "server" | "about";

export function SettingsPanel({ language, onLanguageChange }: Props) {
  const t = messages[language];
  const [section, setSection] = useState<SettingsSection>("preferences");
  const [settings, setSettings] = useState<ServerSettings>({ deploymentId: "", controlUrl: "", relayUrl: "" });
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  const [codex, setCodex] = useState<CodexIntegration | null>(null);
  const [codexBusy, setCodexBusy] = useState(false);
  const [codexError, setCodexError] = useState("");

  useEffect(() => {
    void invoke<ServerSettings>("get_operator_server_settings")
      .then(setSettings)
      .catch((reason) => setError(String(reason)));
    void invoke<CodexIntegration>("codex_integration_status")
      .then(setCodex)
      .catch((reason) => setCodexError(String(reason)));
  }, []);

  async function toggleCodex() {
    if (!codex) return;
    setCodexBusy(true);
    setCodexError("");
    try {
      const updated = await invoke<CodexIntegration>("set_codex_integration", {
        enabled: !codex.enabled,
      });
      setCodex(updated);
    } catch (reason) {
      setCodexError(String(reason));
    } finally {
      setCodexBusy(false);
    }
  }

  async function save() {
    setBusy(true);
    setError("");
    try {
      await invoke("save_operator_server_settings", { settings });
      setSaved(true);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  }

  let codexStatus: string = t.loading;
  let codexAction: string = t.settingsAiEnable;
  if (codex) {
    if (codex.occupied) {
      codexStatus = t.settingsAiConflict;
    } else if (!codex.available) {
      codexStatus = t.settingsAiUnavailable;
    } else {
      codexStatus = codex.enabled ? t.settingsAiEnabled : t.settingsAiDisabled;
    }
    codexAction = codex.enabled ? t.settingsAiDisable : t.settingsAiEnable;
  }

  return (
    <>
      <nav className="surface settings-menu" aria-label={t.nav.settings}>
        <button
          className={section === "preferences" ? "active" : ""}
          aria-current={section === "preferences" ? "page" : undefined}
          onClick={() => setSection("preferences")}
        >
          <Languages size={17} />
          <span>{t.settingsPreferences}</span>
        </button>
        <button
          className={section === "ai" ? "active" : ""}
          aria-current={section === "ai" ? "page" : undefined}
          onClick={() => setSection("ai")}
        >
          <Bot size={17} />
          <span>{t.settingsAi}</span>
        </button>
        <button
          className={section === "server" ? "active" : ""}
          aria-current={section === "server" ? "page" : undefined}
          onClick={() => setSection("server")}
        >
          <Server size={17} />
          <span>{t.settingsConnection}</span>
        </button>
        <button
          className={section === "about" ? "active" : ""}
          aria-current={section === "about" ? "page" : undefined}
          onClick={() => setSection("about")}
        >
          <Info size={17} />
          <span>{t.settingsAbout}</span>
        </button>
      </nav>

      <section className="surface settings-detail">
      {section === "preferences" && <div className="settings-preferences">
        <div className="surface-kicker"><Languages size={15} /> {t.settingsPreferences}</div>
        <h2>{t.language}</h2>
        <p>{t.languageHint}</p>
        <select className="settings-language" aria-label={t.language} value={language}
          onChange={(event) => onLanguageChange(event.target.value as Language)}>
          <option value="zh-CN">简体中文</option>
          <option value="zh-TW">繁體中文</option>
          <option value="en">English</option>
        </select>
      </div>}

      {section === "ai" && <div className="settings-ai">
        <div className="surface-kicker"><Bot size={15} /> {t.settingsAi}</div>
        <h2>{t.settingsAiTitle}</h2>
        <p>{t.settingsAiHint}</p>
        <div className="settings-agent-row">
          <div>
            <strong>Codex</strong>
            <span>{codexStatus}</span>
          </div>
          <button className={codex?.enabled ? "quiet-button" : "primary-button"} disabled={!codex?.available || codex.occupied || codexBusy} onClick={() => void toggleCodex()}>
            {codexBusy ? t.loading : codexAction}
          </button>
        </div>
        {codexError && <p className="settings-error" role="alert">{codexError}</p>}
        <p className="settings-scope-note">{t.settingsAiNote}</p>
      </div>}

      {section === "server" && <div className="settings-server">
        <div className="surface-kicker"><Server size={15} /> {t.settingsConnection}</div>
        <h2>{t.settingsServerTitle}</h2>
        <p>{t.settingsServerHint}</p>
        <div className="settings-fields">
          <label><span className="field-label">{t.settingsDeployment}</span><input value={settings.deploymentId} spellCheck={false} onChange={(event) => { setSettings({ ...settings, deploymentId: event.target.value }); setSaved(false); }} /></label>
          <label><span className="field-label">{t.settingsControl}</span><input value={settings.controlUrl} spellCheck={false} onChange={(event) => { setSettings({ ...settings, controlUrl: event.target.value }); setSaved(false); }} /></label>
          <label><span className="field-label">{t.settingsRelay}</span><input value={settings.relayUrl} spellCheck={false} onChange={(event) => { setSettings({ ...settings, relayUrl: event.target.value }); setSaved(false); }} /></label>
        </div>
        <p className="settings-scope-note">{t.settingsScopeNote}</p>
        {error && <p className="settings-error" role="alert">{error}</p>}
        <div className="settings-actions">
          <button className="primary-button" disabled={busy} onClick={() => void save()}>{busy ? t.settingsSaving : t.settingsSave}<ArrowRight size={16} /></button>
          {saved && <button className="quiet-button" onClick={() => void invoke("restart_desktop")}><RotateCw size={15} />{t.settingsRestart}</button>}
        </div>
      </div>}
      {section === "about" && <div className="settings-about">
        <div className="surface-kicker"><Info size={15} /> {t.settingsAbout}</div>
        <h2>Pixels Agent Bridge</h2>
        <p>v0.1.0</p>
        <p>{t.iconAttribution}</p>
      </div>}
      </section>
    </>
  );
}
