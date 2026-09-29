import { useCallback, useEffect, useLayoutEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Check, Copy, Eye, EyeOff, List, Minus, Monitor, MonitorSmartphone, Moon, Settings2, Sun, UserRound, X } from "lucide-react";
import { initialLanguage, messages, type Language } from "./i18n";
import { OperatorPanel } from "./OperatorPanel";
import { SettingsPanel } from "./SettingsPanel";
import brand from "./assets/brand.svg";
import { formatDeviceCode } from "./deviceCode";
import "./App.css";
import "./theme.css";

export type View = "home" | "remote" | "activity" | "me" | "settings";
type Theme = "light" | "dark";

type DeviceStatus = {
  device_code: string;
  device_id: string;
  temporary_password: string;
  executor_running: boolean;
  control_phase: string;
};

const navItems = [
  { id: "home", icon: Monitor },
  { id: "remote", icon: MonitorSmartphone },
  { id: "activity", icon: List },
  { id: "me", icon: UserRound },
  { id: "settings", icon: Settings2 },
] satisfies { id: View; icon: typeof Monitor }[];

function App() {
  const [language, setLanguage] = useState<Language>(initialLanguage);
  const [theme, setTheme] = useState<Theme>(
    () => window.localStorage.getItem("pab.theme") === "dark" ? "dark" : "light",
  );
  const [view, setView] = useState<View>("home");
  const [device, setDevice] = useState<DeviceStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showPassword, setShowPassword] = useState(true);
  const [copiedField, setCopiedField] = useState<"code" | "password" | null>(null);
  const t = messages[language];
  const appWindow = getCurrentWindow();

  useEffect(() => {
    const currentWindow = getCurrentWindow();
    void (async () => {
      await currentWindow.setMaximizable(false);
      await currentWindow.setResizable(false);
    })().catch((cause) => console.error("Could not lock the window size", cause));
  }, []);

  useEffect(() => {
    const suppressWebviewMenu = (event: MouseEvent) => event.preventDefault();
    document.addEventListener("contextmenu", suppressWebviewMenu);
    return () => document.removeEventListener("contextmenu", suppressWebviewMenu);
  }, []);

  useEffect(() => {
    document.documentElement.lang = language;
    window.localStorage.setItem("pab.language", language);
  }, [language]);

  useLayoutEffect(() => {
    document.documentElement.dataset.theme = theme;
    window.localStorage.setItem("pab.theme", theme);
  }, [theme]);

  useEffect(() => {
    if (!copiedField) return;
    const timer = window.setTimeout(() => setCopiedField(null), 1800);
    return () => window.clearTimeout(timer);
  }, [copiedField]);

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

  async function copy(value: string, field: "code" | "password") {
    try {
      await navigator.clipboard.writeText(value);
      setCopiedField(field);
      setError("");
    } catch {
      setError(t.copyFailed);
    }
  }

  const phase = (() => {
    switch (device?.control_phase?.toLowerCase()) {
      case "authenticated": return t.phaseConnected;
      case "connecting":
      case "reconnecting": return t.phaseConnecting;
      case "disconnected":
      case "stopped": return t.phaseDisconnected;
      default: return t.unknown;
    }
  })();
  const controlConnected = device?.executor_running
    && device.control_phase.toLowerCase() === "authenticated";

  return (
    <div className="window-frame">
      <div
        className="titlebar"
        onMouseDown={(event) => {
          if (event.button !== 0 || event.detail !== 1) return;
          if ((event.target as HTMLElement).closest(".window-controls")) return;
          event.preventDefault();
          void appWindow.startDragging();
        }}
      >
        <div className="titlebar-identity">
          <img className="brand-mark" src={brand} alt="" />
          <span>Pixels Agent Bridge</span>
        </div>
        <div className="window-controls">
          <button
            aria-label={theme === "dark" ? t.switchToLight : t.switchToDark}
            title={theme === "dark" ? t.switchToLight : t.switchToDark}
            aria-pressed={theme === "dark"}
            onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
          >
            {theme === "dark" ? <Sun /> : <Moon />}
          </button>
          <button aria-label={t.minimize} title={t.minimize} onClick={() => void appWindow.minimize()}><Minus /></button>
          <button className="close" aria-label={t.close} title={t.close} onClick={() => void appWindow.close()}><X /></button>
        </div>
      </div>

      <div className="app-body">
        <aside className="sidebar">
          <nav aria-label={t.navigation}>
            {navItems.map(({ id, icon: Icon }) => (
              <button key={id} className={`nav-item ${view === id ? "selected" : ""}`} onClick={() => { setView(id); setError(""); }}>
                <span className="nav-icon" aria-hidden="true"><Icon size={18} strokeWidth={1.8} /></span>
                <span>{t.nav[id]}</span>
              </button>
            ))}
          </nav>
          <div className="sidebar-spacer" />
          <div className="sidebar-status-group">
            <div className="sidebar-status">
              <span className={`status-dot ${device?.executor_running ? "online" : ""}`} />
              <span>{loading ? t.loading : !device ? t.operatorMode : device.executor_running ? t.running : t.stopped}</span>
            </div>
            <div className="sidebar-status">
              <span className={`status-dot ${controlConnected ? "online" : ""}`} />
              <span>{t.controlConnection}: {device ? phase : t.operatorMode}</span>
            </div>
          </div>
        </aside>

        <main className="workspace">
          {error && <div className="toast error" role="status">{error}</div>}

          <div className={`page-content page-${view}`}>
            {view === "home" && (
              <section className="surface home-device">
                {device ? (
                  <div className="home-credentials">
                    <div className="home-credential">
                      <span className="code-label">{t.deviceCode}</span>
                      <div className="home-credential-value">
                        <strong>{formatDeviceCode(device.device_code)}</strong>
                        <button className="credential-icon-button" aria-label={`${t.copy} ${t.deviceCode}`} title={copiedField === "code" ? t.copied : t.copy} onClick={() => void copy(device.device_code, "code")}>
                          {copiedField === "code" ? <Check size={17} /> : <Copy size={17} />}
                        </button>
                      </div>
                    </div>
                    <div className="home-credential">
                      <span className="code-label">{t.temporaryPassword}</span>
                      <div className="home-credential-value">
                        <strong>{showPassword ? device.temporary_password : "••••••••"}</strong>
                        <button className="credential-icon-button" aria-label={showPassword ? t.hide : t.show} title={showPassword ? t.hide : t.show} onClick={() => setShowPassword(!showPassword)}>
                          {showPassword ? <EyeOff size={17} /> : <Eye size={17} />}
                        </button>
                        <button className="credential-icon-button" aria-label={`${t.copy} ${t.temporaryPassword}`} title={copiedField === "password" ? t.copied : t.copy} onClick={() => void copy(device.temporary_password, "password")}>
                          {copiedField === "password" ? <Check size={17} /> : <Copy size={17} />}
                        </button>
                      </div>
                    </div>
                  </div>
                ) : <div className="empty-device">{t.localUnavailable}</div>}
              </section>
            )}

            {view === "settings" ? <SettingsPanel language={language} onLanguageChange={setLanguage} /> : <OperatorPanel language={language} view={view} onOpenRemote={() => setView("remote")} />}
          </div>
        </main>
      </div>
    </div>
  );
}

export default App;
