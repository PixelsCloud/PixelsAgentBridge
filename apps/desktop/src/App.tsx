import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { initialLanguage, messages, type Language } from "./i18n";
import { OperatorPanel } from "./OperatorPanel";
import "./App.css";

export type View = "home" | "remote" | "activity" | "ownership";

type DeviceStatus = {
  device_code: string;
  device_id: string;
  temporary_password: string;
  executor_running: boolean;
  control_phase: string;
};

const navItems: { id: View; icon: string }[] = [
  { id: "home", icon: "⌂" },
  { id: "remote", icon: "↗" },
  { id: "activity", icon: "≡" },
  { id: "ownership", icon: "◇" },
];

function App() {
  const [language, setLanguage] = useState<Language>(initialLanguage);
  const [view, setView] = useState<View>("home");
  const [device, setDevice] = useState<DeviceStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [claimId, setClaimId] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [approving, setApproving] = useState(false);
  const t = messages[language];
  const appWindow = getCurrentWindow();

  useEffect(() => {
    document.documentElement.lang = language;
    window.localStorage.setItem("pab.language", language);
  }, [language]);

  const refresh = useCallback(async () => {
    try {
      setDevice(await invoke<DeviceStatus>("device_status"));
    } catch {
      setDevice(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    let active = true;
    const unlisteners: (() => void)[] = [];
    void listen<DeviceStatus>("local-device-status", (event) => {
      setDevice(event.payload);
      setLoading(false);
    }).then((unlisten) => active ? unlisteners.push(unlisten) : unlisten());
    void listen("local-device-offline", () => void refresh()).then((unlisten) => active ? unlisteners.push(unlisten) : unlisten());
    return () => {
      active = false;
      window.clearInterval(timer);
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [refresh]);

  async function copy(value: string, label: string) {
    try {
      await navigator.clipboard.writeText(value);
      setNotice(language === "zh-CN" ? `${label}${t.copied}` : `${label} ${t.copied}`);
      setError("");
    } catch {
      setError(t.copyFailed);
    }
  }

  async function approve() {
    if (!claimId.trim() || !confirmed) {
      setError(t.claimMissing);
      return;
    }
    setApproving(true);
    setNotice("");
    try {
      await invoke("approve_claim", { claimId: claimId.trim() });
      setNotice(t.approved);
      setError("");
      setClaimId("");
      setConfirmed(false);
    } catch {
      setError(t.approvalFailed);
    } finally {
      setApproving(false);
    }
  }

  const phase = (() => {
    switch (device?.control_phase?.toLowerCase()) {
      case "connected": return t.phaseConnected;
      case "connecting": return t.phaseConnecting;
      case "disconnected": return t.phaseDisconnected;
      default: return t.unknown;
    }
  })();

  return (
    <div className="window-frame">
      <div className="titlebar" data-tauri-drag-region>
        <div className="titlebar-identity" data-tauri-drag-region>
          <span className="brand-mark">P</span>
          <span data-tauri-drag-region>Pixels Agent Bridge</span>
        </div>
        <div className="window-controls">
          <button aria-label={t.minimize} title={t.minimize} onClick={() => void appWindow.minimize()}><svg viewBox="0 0 16 16"><path d="M3 8h10" /></svg></button>
          <button aria-label={t.maximize} title={t.maximize} onClick={() => void appWindow.toggleMaximize()}><svg viewBox="0 0 16 16"><rect x="3" y="3" width="10" height="10" rx="1" /></svg></button>
          <button className="close" aria-label={t.close} title={t.close} onClick={() => void appWindow.close()}><svg viewBox="0 0 16 16"><path d="m4 4 8 8M12 4l-8 8" /></svg></button>
        </div>
      </div>

      <div className="app-body">
        <aside className="sidebar">
          <div className="sidebar-heading">WORKSPACE</div>
          <nav aria-label={t.navigation}>
            {navItems.map(({ id, icon }) => (
              <button key={id} className={`nav-item ${view === id ? "selected" : ""}`} onClick={() => { setView(id); setError(""); setNotice(""); }}>
                <span className="nav-icon" aria-hidden="true">{icon}</span>
                <span>{t.nav[id]}</span>
              </button>
            ))}
          </nav>
          <div className="sidebar-spacer" />
          <div className="sidebar-status">
            <span className={`status-dot ${device?.executor_running ? "online" : ""}`} />
            <span>{loading ? t.loading : device?.executor_running ? t.running : t.stopped}</span>
          </div>
          <div className="language-switch" role="group" aria-label={t.language}>
            <button className={language === "zh-CN" ? "active" : ""} onClick={() => setLanguage("zh-CN")}>中文</button>
            <button className={language === "en" ? "active" : ""} onClick={() => setLanguage("en")}>EN</button>
          </div>
        </aside>

        <main className="workspace">
          <header className="workspace-header">
            <div>
              <div className="eyebrow">PIXELS / {t.nav[view].toUpperCase()}</div>
              <h1>{t.pageTitles[view]}</h1>
              <p>{t.pageDescriptions[view]}</p>
            </div>
            <span className="header-pill"><span className={`status-dot ${device?.executor_running ? "online" : ""}`} />{device?.executor_running ? t.phaseConnected : t.phaseDisconnected}</span>
          </header>

          {(error || notice) && <div className={error ? "toast error" : "toast success"} role="status">{error || notice}</div>}

          <div className={`page-content page-${view}`}>
            {view === "home" && (
              <>
                <section className="surface home-device">
                  <div className="surface-kicker">01 / {t.thisDevice}</div>
                  <div className="surface-topline"><h2>{t.accessTitle}</h2><span className="phase-label">{t.controlConnection}: {phase}</span></div>
                  {device ? (
                    <>
                      <div className="code-label">{t.deviceCode}</div>
                      <div className="hero-code">{device.device_code}</div>
                      <button className="quiet-button" onClick={() => void copy(device.device_code, t.deviceCode)}>{t.copyCode} ↗</button>
                      <div className="device-divider" />
                      <div className="password-line">
                        <div><span className="code-label">{t.temporaryPassword}</span><strong>{showPassword ? device.temporary_password : "•••• •••• ••••"}</strong></div>
                        <div className="inline-actions">
                          <button className="icon-text-button" onClick={() => setShowPassword(!showPassword)}>{showPassword ? t.hide : t.show}</button>
                          <button className="icon-text-button" onClick={() => void copy(device.temporary_password, t.temporaryPassword)}>{t.copy}</button>
                        </div>
                      </div>
                    </>
                  ) : <div className="empty-device">{t.localUnavailable}</div>}
                </section>
                <section className="surface home-next">
                  <div className="surface-kicker">02 / {t.nextStep}</div>
                  <div className="next-illustration" aria-hidden="true"><span>◉</span><i /><span>↗</span></div>
                  <h2>{t.remoteTitle}</h2>
                  <p>{t.remoteDescription}</p>
                  <button className="primary-button" onClick={() => setView("remote")}>{t.openRemote}<span>→</span></button>
                </section>
              </>
            )}

            {view === "ownership" && (
              <section className="surface claim-approval">
                <div className="surface-kicker">02 / {t.thisDevice}</div>
                <h2>{t.claimTitle}</h2>
                <p>{t.claimDescription}</p>
                <label className="field-label" htmlFor="claim-id">{t.claimId}</label>
                <input id="claim-id" value={claimId} onChange={(event) => setClaimId(event.target.value)} placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" spellCheck={false} />
                <label className="check-row"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />{t.claimConfirmation}</label>
                <button className="primary-button" disabled={approving || !device} onClick={() => void approve()}>{approving ? t.approving : t.approve}<span>→</span></button>
              </section>
            )}
            <OperatorPanel language={language} view={view} />
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;
