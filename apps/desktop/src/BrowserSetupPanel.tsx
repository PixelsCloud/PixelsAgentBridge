import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Modal } from "antd";
import { Globe2 } from "lucide-react";
import { messages, type Language } from "./i18n";

type RuntimeInfo = { ready: boolean; chromeAvailable: boolean; edgeAvailable: boolean };
type ProfileStatus = { signedIn: boolean; userId: string | null; chromeSaved: boolean; edgeSaved: boolean };

export function BrowserSetupPanel({ language }: { language: Language }) {
  const t = messages[language];
  const [info, setInfo] = useState<RuntimeInfo | null>(null);
  const [profiles, setProfiles] = useState<ProfileStatus | null>(null);
  const [clearing, setClearing] = useState<string | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    void invoke<RuntimeInfo>("browser_runtime_info").then(setInfo).catch(reason => setError(String(reason)));
    void invoke<ProfileStatus>("browser_profile_status").then(setProfiles).catch(reason => setError(String(reason)));
  }, []);
  function clear(browser: "chrome" | "edge") {
    const expectedUserId = profiles?.userId;
    if (!expectedUserId) return;
    Modal.confirm({
      title: t.browserProfileClearTitle,
      content: t.browserProfileClearConfirm.replace("{browser}", browser === "chrome" ? "Chrome" : "Edge"),
      okText: t.browserProfileClear,
      okButtonProps: { danger: true },
      cancelText: t.cancel,
      onOk: async () => {
        setClearing(browser);
        setError("");
        try {
          setProfiles(await invoke<ProfileStatus>("browser_profile_clear", { browser, expectedUserId }));
        } catch (reason) {
          setError(String(reason));
        } finally {
          setClearing(null);
        }
      },
    });
  }
  return <div className="settings-browser">
    <div className="surface-kicker"><Globe2 size={15} /> {t.settingsBrowser}</div>
    <h2>{t.browserSetupTitle}</h2>
    <p>{t.browserSetupIntro}</p>
    {error && <Alert type="error" showIcon title={error} />}
    {info && !info.ready && <Alert type="warning" showIcon title={t.browserRuntimeMissing} />}
    {info?.ready && !info.chromeAvailable && !info.edgeAvailable && <Alert type="warning" showIcon title={t.browserNotFound} />}
    {info?.ready && (info.chromeAvailable || info.edgeAvailable) && <>
      <p className="settings-scope-note">{t.browserSetupConsent}</p>
      <p className="settings-scope-note">{t.browserSetupLogin}</p>
    </>}
    {profiles?.signedIn && (profiles.chromeSaved || profiles.edgeSaved) && <div className="browser-profile-actions">
      <h3>{t.browserProfileTitle}</h3>
      <p className="settings-scope-note">{t.browserProfileHint}</p>
      {profiles.chromeSaved && <Button danger loading={clearing === "chrome"} onClick={() => clear("chrome")}>{t.browserProfileClear} Chrome</Button>}
      {profiles.edgeSaved && <Button danger loading={clearing === "edge"} onClick={() => clear("edge")}>{t.browserProfileClear} Edge</Button>}
    </div>}
  </div>;
}
