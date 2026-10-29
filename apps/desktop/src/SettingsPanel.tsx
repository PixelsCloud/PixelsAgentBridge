import { userFacingError } from "./userFacingError";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Menu, Select } from "antd";
import { ArrowRight, Bot, Info, Languages, RotateCw, Server, ShieldCheck, Wrench } from "lucide-react";
import { messages, type Language } from "./i18n";
import { McpToolSettingsPanel } from "./McpToolSettingsPanel";
import { MacosPermissionsPanel } from "./MacosPermissionsPanel";
import { macosPermissionText } from "./macosPermissionText";
import { AgentIntegrationsPanel } from "./AgentIntegrationsPanel";
import { UpdatePanel } from "./UpdatePanel";
import { ServerAddressField } from "./ServerAddressField";
import { ProjectRepositoryButton } from "./ProjectRepositoryButton";

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

export type SettingsSection = "preferences" | "permissions" | "ai" | "server" | "tools" | "about";

export function SettingsPanel({ language, onLanguageChange, section, onSectionChange: setSection }: Props) {
  const t = messages[language];
  const [settings, setSettings] = useState<ServerSettings>({ controlUrl: "", relayUrl: "" });
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState("");
  const [isMac, setIsMac] = useState(false);


  useEffect(() => {
    let active = true;
    void invoke<{ screenRecording: boolean } | null>("macos_permissions").then(value => {
      if (active) setIsMac(typeof value?.screenRecording === "boolean");
    }).catch(() => {});
    return () => { active = false; };
  }, []);

  useEffect(() => {
    void invoke<ServerSettings>("get_operator_server_settings")
      .then(value => { setSettings(value); setLoaded(true); })
      .catch((reason) => setError(String(reason)));

  }, []);

  async function save() {
    setBusy(true);
    setError("");
    try {
      const normalized = { controlUrl: settings.controlUrl.trim(), relayUrl: settings.relayUrl.trim() };
      if (Boolean(normalized.controlUrl) !== Boolean(normalized.relayUrl)) {
        setError(t.settingsServerPairRequired);
        return;
      }
      await invoke("save_operator_server_settings", { settings: normalized });
      setSettings(normalized);
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
            ...(isMac ? [{ key: "permissions", icon: <ShieldCheck size={17} />, label: macosPermissionText[language].title }] : []),
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
            { value: "en", label: "English" },
          ]} />
      </div>}

      {section === "permissions" && <MacosPermissionsPanel language={language} />}

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
          <ServerAddressField kind="control" language={language} label={t.settingsControl} disabled={!loaded || busy} value={settings.controlUrl}
            onChange={value => { setSettings(current => ({ ...current, controlUrl: value })); setSaved(false); }} />
          <ServerAddressField kind="relay" language={language} label={t.settingsRelay} disabled={!loaded || busy} value={settings.relayUrl}
            onChange={value => { setSettings(current => ({ ...current, relayUrl: value })); setSaved(false); }} />
        </div>
        <p className="settings-scope-note">{t.settingsScopeNote}</p>
        {error && <p className="settings-error" role="alert">{userFacingError(error, language)}</p>}
        <div className="settings-actions">
          <Button type="primary" disabled={!loaded} loading={busy} icon={<ArrowRight size={16} />} iconPlacement="end" onClick={() => void save()}>{t.settingsSave}</Button>
          {saved && <Button type="link" icon={<RotateCw size={15} />} onClick={() => void invoke("restart_desktop")}>{t.settingsRestart}</Button>}
        </div>
      </div>}
      {section === "tools" && <McpToolSettingsPanel language={language} />}
      {section === "about" && <div className="settings-about">
        <div className="surface-kicker"><Info size={15} /> {t.settingsAbout}</div>
        <div className="settings-about-product">
          <h2>Pixels Agent Bridge</h2>
          <span className="settings-about-version">V{__PAB_RELEASE_VERSION__}</span>
          <ProjectRepositoryButton language={language} />
        </div>
        <p className="settings-about-intro">{t.settingsAboutIntro}</p>
        <UpdatePanel language={language} />
      </div>}
      </section>
    </>
  );
}
