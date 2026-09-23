import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { initialLanguage, messages, type Language } from "./i18n";
import { OperatorPanel } from "./OperatorPanel";
import "./App.css";

type DeviceStatus = {
  device_code: string;
  device_id: string;
  temporary_password: string;
  executor_running: boolean;
  control_phase: string;
};

function App() {
  const [language, setLanguage] = useState<Language>(initialLanguage);
  const [device, setDevice] = useState<DeviceStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [claimId, setClaimId] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [approving, setApproving] = useState(false);
  const t = messages[language];

  useEffect(() => {
    document.documentElement.lang = language;
    window.localStorage.setItem("pab.language", language);
  }, [language]);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<DeviceStatus>("device_status");
      setDevice(next);
      setError("");
    } catch {
      setDevice(null);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 3000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  function changeLanguage(next: Language) {
    setLanguage(next);
    setError("");
    setNotice("");
  }

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
    <main className="app-shell">
      <header className="app-header">
        <div>
          <span className="eyebrow">PIXELS AGENT BRIDGE</span>
          <h1>{t.title}</h1>
          <p>{t.introduction}</p>
        </div>
        <div className="header-actions">
          <label className="language-select">
            <span>{t.language}</span>
            <select
              aria-label={t.language}
              value={language}
              onChange={(event) => changeLanguage(event.target.value as Language)}
            >
              <option value="zh-CN">简体中文</option>
              <option value="en">English</option>
            </select>
          </label>
          <span className={device?.executor_running ? "status online" : "status"}>
            {loading ? t.loading : device?.executor_running ? t.running : t.stopped}
          </span>
        </div>
      </header>

      {error && <div className="alert" role="alert">{error}</div>}
      {notice && <div className="notice" role="status">{notice}</div>}

      <OperatorPanel language={language} />

      <section className="card access-card">
        <div className="section-heading">
          <div>
            <h2>{t.accessTitle}</h2>
            <p>{t.accessDescription}</p>
          </div>
          <span className="phase">{t.controlConnection}: {phase}</span>
        </div>

        {!loading && !device && <p>{t.localUnavailable}</p>}

        {device && <div className="field">
          <label>{t.deviceCode}</label>
          <div className="value-row">
            <strong className="device-code">{device?.device_code ?? "·········"}</strong>
            <button className="secondary" disabled={!device} onClick={() => void copy(device?.device_code ?? "", t.deviceCode)}>{t.copy}</button>
          </div>
        </div>}

        {device && <div className="field">
          <label>{t.temporaryPassword}</label>
          <div className="value-row">
            <strong className="password-value">{showPassword ? device?.temporary_password ?? "" : "••••••••••••"}</strong>
            <button className="secondary" disabled={!device} onClick={() => setShowPassword(!showPassword)}>
              {showPassword ? t.hide : t.show}
            </button>
            <button className="secondary" disabled={!device} onClick={() => void copy(device?.temporary_password ?? "", t.temporaryPassword)}>{t.copy}</button>
          </div>
        </div>}
        {device && <p className="footnote">{t.accessFootnote}</p>}
      </section>

      {device && <section className="card">
        <div className="section-heading">
          <div>
            <h2>{t.claimTitle}</h2>
            <p>{t.claimDescription}</p>
          </div>
        </div>
        <label htmlFor="claim-id">{t.claimId}</label>
        <input
          id="claim-id"
          value={claimId}
          onChange={(event) => setClaimId(event.target.value)}
          placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
          spellCheck={false}
        />
        <label className="check-row">
          <input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
          {t.claimConfirmation}
        </label>
        <button disabled={approving || !device} onClick={() => void approve()}>
          {approving ? t.approving : t.approve}
        </button>
      </section>}
    </main>
  );
}

export default App;
