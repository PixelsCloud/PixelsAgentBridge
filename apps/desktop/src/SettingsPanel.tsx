import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input, Menu, Select } from "antd";
import { ArrowRight, Bot, Info, Languages, RotateCw, Server, Wrench } from "lucide-react";
import { messages, type Language } from "./i18n";
import { McpToolSettingsPanel } from "./McpToolSettingsPanel";
import { MacosPermissionsPanel } from "./MacosPermissionsPanel";
import { AgentIntegrationsPanel } from "./AgentIntegrationsPanel";
import { UpdatePanel } from "./UpdatePanel";

type ServerSettings = {
  controlUrl: string;
  relayUrl: string;
};

type Props = {
  language: Language;
  onLanguageChange: (value: Language) => void;
  section: SettingsSection;
  onSectionChange: (value: SettingsSection) => void;
};

export type SettingsSection = "preferences" | "ai" | "server" | "tools" | "about";

export function SettingsPanel({ language, onLanguageChange, section, onSectionChange: setSection }: Props) {
  const t = messages[language];
  const [settings, setSettings] = useState<ServerSettings>({ controlUrl: "", relayUrl: "" });
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");


  useEffect(() => {
    void invoke<ServerSettings>("get_operator_server_settings")
      .then(setSettings)
      .catch((reason) => setError(String(reason)));

  }, []);

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

  return (
    <>
      <nav className="surface settings-menu" aria-label={t.nav.settings}>
        <Menu mode="inline" selectedKeys={[section]} onClick={({ key }) => setSection(key as SettingsSection)}
          items={[
            { key: "preferences", icon: <Languages size={17} />, label: t.settingsPreferences },
            { key: "ai", icon: <Bot size={17} />, label: t.settingsAi },
            { key: "tools", icon: <Wrench size={17} />, label: t.settingsTools },
            { key: "server", icon: <Server size={17} />, label: t.settingsConnection },
            { key: "about", icon: <Info size={17} />, label: t.settingsAbout },
          ]} />
      </nav>

      <section className="surface settings-detail">
      {section === "preferences" && <div className="settings-preferences">
        <div className="surface-kicker"><Languages size={15} /> {t.settingsPreferences}</div>
        <h2>{t.language}</h2>
        <p>{t.languageHint}</p>
        <Select className="settings-language" aria-label={t.language} value={language}
          onSelect={(value: Language) => onLanguageChange(value)}
          options={[
            { value: "zh-CN", label: "简体中文" },
            { value: "zh-TW", label: "繁體中文" },
            { value: "en", label: "English" },
          ]} />
        <MacosPermissionsPanel language={language} />
      </div>}

      {section === "ai" && <div className="settings-ai">
        <div className="surface-kicker"><Bot size={15} /> {t.settingsAi}</div>
        <h2>{t.settingsAiTitle}</h2>
        <p>{t.settingsAiHint}</p>
        <AgentIntegrationsPanel language={language} />
        <p className="settings-scope-note">{t.settingsAiNote}</p>
      </div>}

      {section === "server" && <div className="settings-server">
        <div className="surface-kicker"><Server size={15} /> {t.settingsConnection}</div>
        <h2>{t.settingsServerTitle}</h2>
        <p>{t.settingsServerHint}</p>
        <div className="settings-fields">
          <label><span className="field-label">{t.settingsControl}</span><Input value={settings.controlUrl} spellCheck={false} onChange={(event) => { setSettings({ ...settings, controlUrl: event.target.value }); setSaved(false); }} /></label>
          <label><span className="field-label">{t.settingsRelay}</span><Input value={settings.relayUrl} spellCheck={false} onChange={(event) => { setSettings({ ...settings, relayUrl: event.target.value }); setSaved(false); }} /></label>
        </div>
        <p className="settings-scope-note">{t.settingsScopeNote}</p>
        {error && <p className="settings-error" role="alert">{error}</p>}
        <div className="settings-actions">
          <Button type="primary" loading={busy} icon={<ArrowRight size={16} />} iconPlacement="end" onClick={() => void save()}>{t.settingsSave}</Button>
          {saved && <Button type="link" icon={<RotateCw size={15} />} onClick={() => void invoke("restart_desktop")}>{t.settingsRestart}</Button>}
        </div>
      </div>}
      {section === "tools" && <McpToolSettingsPanel language={language} />}
      {section === "about" && <div className="settings-about">
        <div className="surface-kicker"><Info size={15} /> {t.settingsAbout}</div>
        <div className="settings-about-product">
          <h2>Pixels Agent Bridge</h2>
          <span className="settings-about-version">V{__PAB_RELEASE_VERSION__}</span>
        </div>
        <p className="settings-about-intro">{t.settingsAboutIntro}</p>
        <UpdatePanel language={language} />
      </div>}
      </section>
    </>
  );
}
