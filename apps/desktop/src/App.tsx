import { userFacingError } from "./userFacingError";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { App as AntdApp, Avatar, Button, ConfigProvider, Menu, theme as antdTheme } from "antd";
import enUS from "antd/locale/en_US";
import zhCN from "antd/locale/zh_CN";
import { Check, Copy, List, Minus, Monitor, MonitorSmartphone, Moon, Plug, RotateCw, Settings2, Sun, UserRound, X } from "lucide-react";
import { AppRefreshContext, useAppRefreshRegistry } from "./appRefresh";
import { initialLanguage, messages, type Language } from "./i18n";
import { OperatorPanel } from "./OperatorPanel";
import { SettingsPanel, type SettingsSection } from "./SettingsPanel";
import { ProjectRepositoryButton } from "./ProjectRepositoryButton";
import { UpdateNotice } from "./UpdateNotice";
import { BrowserHandoffNotice } from "./BrowserHandoffNotice";
import { DesktopAssistanceNotice } from "./DesktopAssistanceNotice";
import { McpConnectionsPanel } from "./McpConnectionsPanel";
import { AccountConnectionPanel, AccountLoginDialog } from "./AccountConnectionPanel";
import type { ScopeStatus } from "./operatorTypes";
import { MacosPermissionStatus, requestMacosPermissionsAtStartup } from "./MacosPermissionsPanel";
import brand from "./assets/brand.svg";
import { formatDeviceCode } from "./deviceCode";
import { DevicePassword } from "./DevicePassword";
import { currentDeviceStatus, type DeviceStatus } from "./localDeviceStatus";
import "./App.css";
import "./PasswordMask.css";
import "./theme.css";

export type View = "home" | "remote" | "mcp" | "activity" | "me" | "settings";
type Theme = "light" | "dark";

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
  const [copiedField, setCopiedField] = useState<"code" | "password" | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const refreshPending = useRef(false);
  const accountRequest = useRef(0);
  const scopeRef = useRef<ScopeStatus | null>(null);
  const refreshRegistry = useAppRefreshRegistry();
  const t = messages[language];
  const appWindow = getCurrentWindow();

  function acceptScope(scope: ScopeStatus | null) {
    accountRequest.current += 1;
    scopeRef.current = scope;
    setActiveScope(scope);
    setAccountLoadFailed(false);
  }

  const refreshAccount = useCallback(async () => {
    const request = ++accountRequest.current;
    try {
      const scope = await invoke<ScopeStatus | null>("operator_current_traffic_scope");
      if (request !== accountRequest.current) return;
      scopeRef.current = scope;
      setActiveScope(scope);
      setAccountLoadFailed(false);
      if (!scope) setView(current => current === "me" ? "home" : current);
    } catch (cause) {
      if (request === accountRequest.current) setAccountLoadFailed(true);
      throw cause;
    }
  }, []);

  useEffect(() => {
    void refreshAccount().catch(() => {}).finally(() => setAccountLoading(false));
    return () => { accountRequest.current += 1; };
  }, [refreshAccount]);

  async function openAccount() {
    if (accountLoading) return;
    let scope = activeScope;
    if (accountLoadFailed) {
      setAccountLoading(true);
      try {
        await refreshAccount();
        scope = scopeRef.current;
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
      const next = await invoke<DeviceStatus>("device_status");
      setDevice((current) => currentDeviceStatus(current, next));
    } catch {
      setDevice(null);
    } finally {
      setLoading(false);
    }
  }, []);

  async function refreshAll() {
    if (refreshPending.current || accountLoading) return;
    refreshPending.current = true;
    setRefreshing(true);
    setError("");
    try {
      const results = await Promise.allSettled([
        refreshAccount(),
        refreshRegistry.refresh(),
        invoke<DeviceStatus>("device_status").then(next => {
          setDevice(current => currentDeviceStatus(current, next));
          setLoading(false);
        }),
      ]);
      if (results.some(result => result.status === "rejected")) setError(t.refreshAllFailed);
    } finally {
      refreshPending.current = false;
      setRefreshing(false);
    }
  }

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    let active = true;
    const unlisteners: (() => void)[] = [];
    void listen<DeviceStatus>("local-device-status", (event) => {
      setDevice((current) => currentDeviceStatus(current, event.payload));
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
      case "unconfigured": return t.serverUnconfigured;
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
      locale={language === "zh-CN" ? zhCN : enUS}
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
    <AppRefreshContext.Provider value={refreshRegistry}>
    <div className="window-frame">
      <BrowserHandoffNotice language={language} />
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
          <span>Pixels Agent Bridge (V{__PAB_RELEASE_VERSION__})</span>
        </div>
        <div className="window-controls">
          <ProjectRepositoryButton language={language} titlebar />
          <button type="button" className={`titlebar-refresh${refreshing ? " is-refreshing" : ""}`}
            aria-label={t.refreshAll} title={refreshing ? t.refreshAllBusy : t.refreshAll}
            aria-busy={refreshing} disabled={refreshing || accountLoading} onClick={() => void refreshAll()}>
            <RotateCw aria-hidden="true" />
          </button>
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
            aria-label={activeScope ? `${t.nav.me}: ${activeScope.username}` : t.accountStartControl}>
            <Avatar size={38} icon={!activeScope ? <UserRound size={20} /> : undefined}>
              {activeScope ? Array.from(activeScope.username)[0]?.toUpperCase() : undefined}
            </Avatar>
            <span className="sidebar-account-info">
              <strong title={activeScope?.username}>{accountLoading ? t.loading : accountLoadFailed ? t.accountLoadFailed : activeScope?.username || t.accountNotSignedIn}</strong>
              <small>{accountLoadFailed ? t.accountRetry : activeScope ? t.accountViewProfile : t.accountStartControl}</small>
            </span>
          </Button>
          <nav aria-label={t.navigation}>
            <Menu className="sidebar-menu" mode="inline" selectedKeys={[view]}
              items={navItems.map(({ id, icon: Icon }) => ({ key: id, label: t.nav[id], icon: <Icon size={18} strokeWidth={1.8} /> }))}
              onClick={({ key }) => { if (key === "me") { void openAccount(); return; } setView(key as View); setError(""); }} />
          </nav>
          <div className="sidebar-spacer" />
          <div className="sidebar-status-group">
            <UpdateNotice language={language} onOpen={() => { setSettingsSection("about"); setView("settings"); setError(""); }} />
            <MacosPermissionStatus language={language} onOpenSettings={() => { setSettingsSection("permissions"); setView("settings"); setError(""); }} />
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
          {error && <div className="toast error" role="status">{userFacingError(error, language)}</div>}

          <div className={`page-content page-${view}`}>
            {view === "home" && (
              <section className="surface home-device">
                {device?.control_phase.toLowerCase() === "unconfigured" ? (
                  <div className="empty-device">{t.deviceServerUnconfigured}</div>
                ) : device ? (
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
                    <DevicePassword language={language} device={device} onError={setError}
                      onRefreshed={(next) => setDevice((current) => currentDeviceStatus(current, next))} />
                  </div>
                ) : <div className="empty-device">{t.localUnavailable}</div>}
              </section>
            )}

            {view === "settings" ? <SettingsPanel language={language} onLanguageChange={changeLanguage} section={settingsSection} onSectionChange={setSettingsSection} />
              : view === "mcp" ? <section className="surface mcp-connections-page"><McpConnectionsPanel language={language} /></section>
                : view === "me" && activeScope ? <AccountConnectionPanel language={language} activeScope={activeScope} onSignOut={() => { acceptScope(null); setView("home"); }} />
                  : <OperatorPanel key={`${activeScope?.userId ?? "guest"}:${activeScope?.revision ?? 0}`} language={language} view={view} signedIn={!!activeScope} onSignIn={() => setLoginOpen(true)} onOpenRemote={() => setView("remote")} />}
          </div>
        </main>
      </div>
      <AccountLoginDialog language={language} open={loginOpen} onClose={() => setLoginOpen(false)} onSignedIn={(scope) => {
        acceptScope(scope);
        setLoginOpen(false);
        setView("me");
      }} />
      <DesktopAssistanceNotice language={language} />
    </div>
    </AppRefreshContext.Provider>
    </AntdApp>
    </ConfigProvider>
  );
}

export default App;
