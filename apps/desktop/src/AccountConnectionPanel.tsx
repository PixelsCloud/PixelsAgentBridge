import { useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ArrowRight, Eye, EyeOff } from "lucide-react";
import { messages, type Language } from "./i18n";
import type { ScopeStatus } from "./operatorTypes";

type Props = {
  language: Language;
  activeScope: ScopeStatus | null;
  onScopeChange: (scope: ScopeStatus | null) => void;
};

export function AccountConnectionPanel({ language, activeScope, onScopeChange }: Props) {
  const t = messages[language];
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  async function connect(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!username.trim() || !password || busy) return;
    setBusy(true);
    setError("");
    try {
      const account = username.trim();
      const status = await invoke<ScopeStatus>("operator_login_account", {
        username: account,
        password,
      });
      setPassword("");
      onScopeChange(status);
    } catch (cause) {
      setError(`${t.accountConnectFailed}: ${String(cause)}`);
    } finally {
      setBusy(false);
    }
  }

  async function disconnect() {
    setBusy(true);
    setError("");
    try {
      await invoke("operator_use_guest_scope");
      setPassword("");
      onScopeChange(null);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="surface home-account">
      <h2>{t.accountConnection}</h2>
      <div className="account-current">
        <span>{t.currentIdentity}</span>
        <strong>{activeScope?.username || t.guestOperator}</strong>
        {activeScope && <small>{t.accountId}: {activeScope.userId}</small>}
      </div>
      {activeScope ? (
        <button className="home-account-disconnect" type="button" disabled={busy} onClick={() => void disconnect()}>
          {t.accountDisconnect}
        </button>
      ) : (
        <form className="home-account-form" onSubmit={(event) => void connect(event)}>
          <label>
            <span className="field-label">{t.accountName}</span>
            <input
              autoComplete="username"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
            />
          </label>
          <div className="home-account-password-field">
            <label className="field-label" htmlFor="home-account-password">{t.accountPassword}</label>
            <span className="home-account-password">
              <input
                id="home-account-password"
                type={showPassword ? "text" : "password"}
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
              <button
                type="button"
                aria-label={showPassword ? t.hide : t.show}
                title={showPassword ? t.hide : t.show}
                onClick={() => setShowPassword((current) => !current)}
              >
                {showPassword ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
            </span>
          </div>
          <button className="primary-button" type="submit" disabled={busy || !username.trim() || !password}>
            {busy ? t.connecting : t.accountConnect}<ArrowRight size={16} />
          </button>
        </form>
      )}
      {error && <div className="inline-error" role="alert">{error}</div>}
    </section>
  );
}
