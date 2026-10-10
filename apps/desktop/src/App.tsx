import { useCallback, useEffect, useLayoutEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { App as AntdApp, Avatar, Button, ConfigProvider, Menu, theme as antdTheme } from "antd";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";
import zhTW from "antd/locale/zh_TW";
import { Check, Copy, Eye, EyeOff, List, Minus, Monitor, MonitorSmartphone, Moon, Plug, Settings2, Sun, UserRound, X } from "lucide-react";
import { initialLanguage, messages, type Language } from "./i18n";
import { OperatorPanel } from "./OperatorPanel";
import { SettingsPanel, type SettingsSection } from "./SettingsPanel";
import { UpdateNotice } from "./UpdateNotice";
import { McpConnectionsPanel } from "./McpConnectionsPanel";
import { AccountConnectionPanel, AccountLoginDialog } from "./AccountConnectionPanel";
import type { ScopeStatus } from "./operatorTypes";
import { MacosPermissionStatus, requestMacosPermissionsAtStartup } from "./MacosPermissionsPanel";
import brand from "./assets/brand.svg";
import { formatDeviceCode } from "./deviceCode";
import "./App.css";
import "./theme.css";

export type View = "home" | "remote" | "mcp" | "activity" | "me" | "settings";
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
  { id: "mcp", icon: Plug },
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
  const [activeScope, setActiveScope] = useState<ScopeStatus | null>(null);
  const [accountLoading, setAccountLoading] = useState(true);
  const [accountLoadFailed, setAccountLoadFailed] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("preferences");
  const [device, setDevice] = useState<DeviceStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [showPassword, setShowPassword] = useState(true);
  const [copiedField, setCopiedField] = useState<"code" | "password" | null>(null);
  const t = messages[language];
  const appWindow = getCurrentWindow();

  useEffect(() => {
    let active = true;
    void invoke<ScopeStatus | null>("operator_current_traffic_scope")
      .then((scope) => { if (active) setActiveScope(scope); })
      .catch(() => { if (active) setAccountLoadFailed(true); })
      .finally(() => { if (active) setAccountLoading(false); });
    return () => { active = false; };
  }, []);

  async function openAccount() {
    if (accountLoading) return;
    let scope = activeScope;
    if (accountLoadFailed) {
      setAccountLoading(true);
      try {
        scope = await invoke<ScopeStatus | null>("operator_current_traffic_scope");
        setActiveScope(scope);
        setAccountLoadFailed(false);
      } catch {
        return;
      } finally {
        setAccountLoading(false);
      }
    }
    setError("");
    if (scope) setView("me");
    else setLoginOpen(true);
  }

  useEffect(() => {
    void requestMacosPermissionsAtStartup().catch((cause) => console.error("Could not request macOS permissions", cause));
  }, []);

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
    void invoke("set_tray_language", { language }).catch((cause) => console.error("Could not update tray language", cause));
  }, [language]);

  function changeLanguage(value: Language) {
    window.localStorage.setItem("pab.language", value);
    setLanguage(value);
  }

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

  const serverStatus = (() => {
    if (!device) return t.serverUnknown;
    if (!device.executor_running) return t.serverDisconnected;
    switch (device?.control_phase?.toLowerCase()) {
      case "authenticated": return t.serverConnected;
      case "connecting":
      case "reconnecting": return t.serverConnecting;
      case "disconnected":
      case "stopped": return t.serverDisconnected;
      default: return t.serverUnknown;
    }
  })();
  const controlConnected = device?.executor_running
    && device.control_phase.toLowerCase() === "authenticated";

  return (
    <ConfigProvider
      locale={language === "zh-CN" ? zhCN : language === "zh-TW" ? zhTW : enUS}
      theme={{
        algorithm: theme === "dark" ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
        token: {
          colorPrimary: theme === "dark" ? "#93d7c2" : "#218574",
          colorSuccess: "#2ca777",
          colorError: "#b9474b",
          colorBgContainer: theme === "dark" ? "#2b3038" : "#ffffff",
          colorBgElevated: theme === "dark" ? "#333a43" : "#ffffff",
          colorBorder: theme === "dark" ? "#434b55" : "#d4e0e5",
          colorText: theme === "dark" ? "#f0f3f5" : "#162637",
          borderRadius: 9,
          fontSize: 12,
          controlHeight: 38,
          fontFamily: 'Inter, "Segoe UI", "Noto Sans CJK SC", system-ui, sans-serif',
        },
        components: {
          Button: {
            primaryColor: theme === "dark" ? "#162637" : "#ffffff",
            ...(theme === "light" ? {
              colorPrimary: "#1e7b6c",
              colorPrimaryHover: "#208372",
              colorPrimaryActive: "#176354",
            } : {}),
          },
          Input: { colorBgContainer: theme === "dark" ? "#242a32" : "#f9fbfc" },
          Select: { colorBgContainer: theme === "dark" ? "#242a32" : "#f9fbfc" },
          Menu: { itemBg: "transparent", itemSelectedBg: theme === "dark" ? "#345047" : "#dff1e9" },
        },
      }}
    >
    <AntdApp className="desktop-ant-app">
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
          <span>Pixels Agent Bridge (v {__PAB_RELEASE_VERSION__})</span>
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
          <button className="close" aria-label={t.hideToTray} title={t.hideToTray} onClick={() => void appWindow.close()}><X /></button>
        </div>
      </div>

      <div className="app-body">
        <aside className="sidebar">
          <Button type="text" className="sidebar-account" onClick={() => void openAccount()} disabled={accountLoading}
            aria-label={activeScope ? `${t.nav.me}: ${activeScope.username}` : t.accountConnect}>
            <Avatar size={38} icon={!activeScope ? <UserRound size={20} /> : undefined}>
              {activeScope ? Array.from(activeScope.username)[0]?.toUpperCase() : undefined}
            </Avatar>
            <span className="sidebar-account-info">
              <strong title={activeScope?.username}>{accountLoading ? t.loading : accountLoadFailed ? t.accountLoadFailed : activeScope?.username || t.accountNotSignedIn}</strong>
              <small>{accountLoadFailed ? t.accountRetry : activeScope ? t.accountViewProfile : t.accountConnect}</small>
            </span>
          </Button>
          <nav aria-label={t.navigation}>
            <Menu className="sidebar-menu" mode="inline" selectedKeys={[view]}
              items={navItems.map(({ id, icon: Icon }) => ({ key: id, label: t.nav[id], icon: <Icon size={18} strokeWidth={1.8} /> }))}
              onClick={({ key }) => { if (key === "me") { void openAccount(); return; } setView(key as View); setError(""); }} />
          </nav>
          <div className="sidebar-spacer" />
          <div className="sidebar-status-group">
            <MacosPermissionStatus language={language} onOpenSettings={() => { setSettingsSection("preferences"); setView("settings"); setError(""); }} />
            <div className="sidebar-status">
              <span className={`status-dot ${device?.executor_running ? "online" : ""}`} />
              <span>{loading ? t.localServiceLoading : !device ? t.localServiceUnknown : device.executor_running ? t.running : t.stopped}</span>
            </div>
            <div className="sidebar-status">
              <span className={`status-dot ${controlConnected ? "online" : ""}`} />
              <span>{serverStatus}</span>
            </div>
          </div>
        </aside>

        <main className="workspace">
          {error && <div className="toast error" role="status">{error}</div>}
          <UpdateNotice language={language} onOpen={() => { setSettingsSection("about"); setView("settings"); setError(""); }} />

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

            {view === "settings" ? <SettingsPanel language={language} onLanguageChange={changeLanguage} section={settingsSection} onSectionChange={setSettingsSection} />
              : view === "mcp" ? <section className="surface mcp-connections-page"><McpConnectionsPanel language={language} /></section>
                : view === "me" && activeScope ? <AccountConnectionPanel language={language} activeScope={activeScope} onSignOut={() => { setActiveScope(null); setView("home"); }} />
                  : <OperatorPanel language={language} view={view} signedIn={!!activeScope} onSignIn={() => setLoginOpen(true)} onOpenRemote={() => setView("remote")} />}
          </div>
        </main>
      </div>
      <AccountLoginDialog language={language} open={loginOpen} onClose={() => setLoginOpen(false)} onSignedIn={(scope) => {
        setActiveScope(scope);
        setLoginOpen(false);
        setView("me");
      }} />
    </div>
    </AntdApp>
    </ConfigProvider>
  );
}

export default App;
