import { useEffect, useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Avatar, Button, Input, Modal, Typography } from "antd";
import { ExternalLink, LogOut } from "lucide-react";
import { messages, type Language } from "./i18n";
import type { ScopeStatus } from "./operatorTypes";
import { DeviceAccountPanel } from "./DeviceAccountPanel";
import { AccountCatalogPanel } from "./AccountCatalogPanel";
import { GithubAccount } from "./GithubAccount";

export function AccountConnectionPanel({ language, activeScope, onSignOut }: {
  language: Language;
  activeScope: ScopeStatus;
  onSignOut: () => void;
}) {
  const t = messages[language];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [openingConsole, setOpeningConsole] = useState(false);

  async function openConsole() {
    if (openingConsole) return;
    setOpeningConsole(true); setError("");
    try { await invoke("open_account_console"); }
    catch { setError(t.accountConsoleOpenFailed); }
    finally { setOpeningConsole(false); }
  }

  async function disconnect() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await invoke("operator_use_guest_scope");
      onSignOut();
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="surface home-account account-page">
      <header className="account-page-heading">
        <h2>{t.nav.me}</h2>
        <Button type="primary" icon={<ExternalLink size={15} />} loading={openingConsole} onClick={() => void openConsole()}>{t.accountOpenConsole}</Button>
      </header>
      <div className="account-profile-card">
        <div className="account-profile-heading">
          <Avatar size={48}>{Array.from(activeScope.username)[0]?.toUpperCase()}</Avatar>
          <div className="account-profile-name"><span>{t.accountName}</span><strong>{activeScope.username}</strong></div>
        </div>
        <div className="account-profile-id"><span>{t.accountId}</span><Typography.Text copyable>{activeScope.userId}</Typography.Text></div>
      </div>
      <div className="account-sections">
        <section className="account-section" aria-label={t.deviceAccountTitle}><DeviceAccountPanel key={activeScope.revision} language={language} accountRevision={activeScope.revision} /></section>
        <section className="account-section" aria-label={t.accountDeviceSync}><AccountCatalogPanel key={`catalog-${activeScope.revision}`} language={language} accountRevision={activeScope.revision} /></section>
      </div>
      <footer className="account-actions">
        <GithubAccount key={activeScope.userId} language={language} signedIn compact disabled={busy} onBusy={setBusy}/>
        <Button className="home-account-disconnect" icon={<LogOut size={14} />} danger loading={busy} onClick={() => void disconnect()}>{t.accountDisconnect}</Button>
      </footer>
      {error && <Alert className="account-error" type="error" showIcon title={error} />}
    </section>
  );
}

export function AccountLoginDialog({ language, open, onClose, onSignedIn }: {
  language: Language;
  open: boolean;
  onClose: () => void;
  onSignedIn: (scope: ScopeStatus) => void;
}) {
  const t = messages[language];
  const [busy, setBusy] = useState(false);
  const [register, setRegister] = useState(false);
  useEffect(() => { if (!open) setRegister(false); }, [open]);
  return <Modal title={register ? t.accountRegister : t.accountConnect} open={open} footer={null} width={400}
    mask={{ closable: false }} keyboard={false} closable={!busy} onCancel={onClose} destroyOnHidden>
    {open && <AccountLoginForm language={language} busy={busy} setBusy={setBusy} register={register} setRegister={setRegister} onSignedIn={onSignedIn} />}
  </Modal>;
}

function AccountLoginForm({ language, busy, setBusy, register, setRegister, onSignedIn }: {
  language: Language;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  register: boolean;
  setRegister: (register: boolean) => void;
  onSignedIn: (scope: ScopeStatus) => void;
}) {
  const t = messages[language];
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState("");
  const passwordMismatch = register && confirmPassword.length > 0 && password !== confirmPassword;
  const normalizedUsername = username.trim().normalize("NFKC");
  const usernameInvalid = register && (Array.from(normalizedUsername).length < 3
    || Array.from(normalizedUsername).length > 64 || /[\p{White_Space}\p{Cc}/\\]/u.test(normalizedUsername));

  async function connect(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!username.trim() || !password || busy || (register && password !== confirmPassword)) return;
    if (usernameInvalid) {
      setError(`${t.accountRegisterFailed}: ${t.accountUsernameInvalid}`);
      return;
    }
    if (register && Array.from(password).length < 8) {
      setError(`${t.accountRegisterFailed}: ${t.accountPasswordTooShort}`);
      return;
    }
    setBusy(true);
    setError("");
    try {
      const status = await invoke<ScopeStatus>(register ? "operator_register_account" : "operator_login_account", { username: username.trim(), password });
      setPassword("");
      setConfirmPassword("");
      onSignedIn(status);
    } catch (cause) {
      const reasons: Record<string, string> = {
        account_username_invalid: t.accountUsernameInvalid,
        account_password_too_short: t.accountPasswordTooShort,
        account_password_too_long: t.accountPasswordTooLong,
        account_registration_invalid: t.accountRegistrationInvalid,
        account_registration_disabled: t.accountRegistrationDisabled,
        account_username_taken: t.accountUsernameTaken,
        account_invalid_credentials: t.accountInvalidCredentials,
        account_access_denied: t.accountAccessDenied,
        account_rate_limited: t.accountRateLimited,
        account_service_unavailable: t.accountServiceUnavailable,
        account_network_timeout: t.accountNetworkTimeout,
        account_dns_failed: t.accountDnsFailed,
        account_tls_failed: t.accountTlsFailed,
        account_proxy_failed: t.accountProxyFailed,
        account_proxy_authentication_required: t.accountProxyAuthentication,
        account_ca_invalid: t.accountCaInvalid,
        account_network_configuration_invalid: t.accountNetworkConfigurationInvalid,
        account_connection_failed: t.accountConnectionFailed,
        "account request failed; cached login was retained": t.accountServiceUnavailable,
      };
      setError(`${register ? t.accountRegisterFailed : t.accountConnectFailed}: ${reasons[String(cause)] ?? String(cause)}`);
    } finally {
      setBusy(false);
    }
  }

  return <form className="account-login-form" onSubmit={(event) => void connect(event)}>
    <label htmlFor="account-login-username">{t.accountName}</label>
    <Input id="account-login-username" autoComplete="username" autoFocus disabled={busy}
      status={usernameInvalid && username.length > 0 ? "error" : undefined}
      aria-invalid={(usernameInvalid && username.length > 0) || undefined}
      aria-describedby={register ? "account-username-hint" : undefined}
      value={username} onChange={(event) => setUsername(event.target.value)} />
    {register && <Typography.Text id="account-username-hint" type={usernameInvalid && username.length > 0 ? "danger" : "secondary"}>{t.accountUsernameInvalid}</Typography.Text>}
    <label htmlFor="account-login-password">{t.accountPassword}</label>
    <Input.Password id="account-login-password" autoComplete={register ? "new-password" : "current-password"} disabled={busy}
      status={passwordMismatch ? "error" : undefined} aria-invalid={passwordMismatch || undefined}
      aria-describedby={register ? `account-password-hint${passwordMismatch ? " account-password-error" : ""}` : undefined}
      value={password} onChange={(event) => setPassword(event.target.value)} />
    {register && <Typography.Text id="account-password-hint" type="secondary">{t.accountPasswordTooShort}</Typography.Text>}
    {register && <><label htmlFor="account-confirm-password">{t.accountConfirmPassword}</label>
      <Input.Password id="account-confirm-password" autoComplete="new-password" disabled={busy}
        status={passwordMismatch ? "error" : undefined} aria-invalid={passwordMismatch || undefined}
        aria-describedby={passwordMismatch ? "account-password-error" : undefined}
        value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} /></>}
    {passwordMismatch && <Typography.Text id="account-password-error" type="danger" role="alert">{t.accountPasswordMismatch}</Typography.Text>}
    {error && <Alert type="error" showIcon title={error} />}
    <Button type="primary" htmlType="submit" loading={busy} disabled={!username.trim() || usernameInvalid || !password || (register && password !== confirmPassword)} block>{register ? t.accountRegister : t.accountConnect}</Button>
    <Button type="link" disabled={busy} onClick={() => { setRegister(!register); setError(""); setPassword(""); setConfirmPassword(""); }}>{register ? t.accountAlreadyRegistered : t.accountRegister}</Button>
    <GithubAccount language={language} disabled={busy} onBusy={setBusy} onSignedIn={onSignedIn}/>
  </form>;
}
