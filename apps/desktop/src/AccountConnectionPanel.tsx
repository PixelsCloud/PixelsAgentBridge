import { useState, type FormEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Avatar, Button, Descriptions, Input, Modal } from "antd";
import { messages, type Language } from "./i18n";
import type { ScopeStatus } from "./operatorTypes";

export function AccountConnectionPanel({ language, activeScope, onSignOut }: {
  language: Language;
  activeScope: ScopeStatus;
  onSignOut: () => void;
}) {
  const t = messages[language];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

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
    <section className="surface home-account">
      <h2>{t.nav.me}</h2>
      <div className="account-profile-heading">
        <Avatar size={56}>{Array.from(activeScope.username)[0]?.toUpperCase()}</Avatar>
        <strong>{activeScope.username}</strong>
      </div>
      <Descriptions column={1} size="small" items={[
        { key: "username", label: t.accountName, children: activeScope.username },
        { key: "id", label: t.accountId, children: activeScope.userId },
      ]} />
      <Button className="home-account-disconnect" loading={busy} onClick={() => void disconnect()}>{t.accountDisconnect}</Button>
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
  return <Modal title={t.accountConnect} open={open} footer={null} width={400}
    mask={{ closable: false }} keyboard={false} closable={!busy} onCancel={onClose} destroyOnHidden>
    {open && <AccountLoginForm language={language} busy={busy} setBusy={setBusy} onSignedIn={onSignedIn} />}
  </Modal>;
}

function AccountLoginForm({ language, busy, setBusy, onSignedIn }: {
  language: Language;
  busy: boolean;
  setBusy: (busy: boolean) => void;
  onSignedIn: (scope: ScopeStatus) => void;
}) {
  const t = messages[language];
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [register, setRegister] = useState(false);
  const [confirmPassword, setConfirmPassword] = useState("");
  const [error, setError] = useState("");

  async function connect(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!username.trim() || !password || busy || (register && password !== confirmPassword)) return;
    setBusy(true);
    setError("");
    try {
      const status = await invoke<ScopeStatus>(register ? "operator_register_account" : "operator_login_account", { username: username.trim(), password });
      setPassword("");
      setConfirmPassword("");
      onSignedIn(status);
    } catch (cause) {
      setError(`${t.accountConnectFailed}: ${String(cause)}`);
    } finally {
      setBusy(false);
    }
  }

  return <form className="account-login-form" onSubmit={(event) => void connect(event)}>
    <label htmlFor="account-login-username">{t.accountName}</label>
    <Input id="account-login-username" autoComplete="username" autoFocus disabled={busy}
      value={username} onChange={(event) => setUsername(event.target.value)} />
    <label htmlFor="account-login-password">{t.accountPassword}</label>
    <Input.Password id="account-login-password" autoComplete={register ? "new-password" : "current-password"} disabled={busy}
      value={password} onChange={(event) => setPassword(event.target.value)} />
    {register && <><label htmlFor="account-confirm-password">{t.accountConfirmPassword}</label>
      <Input.Password id="account-confirm-password" autoComplete="new-password" disabled={busy}
        value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} /></>}
    {error && <Alert type="error" showIcon title={error} />}
    <Button type="primary" htmlType="submit" loading={busy} disabled={!username.trim() || !password || (register && password !== confirmPassword)} block>{register ? t.accountRegister : t.accountConnect}</Button>
    <Button type="link" disabled={busy} onClick={() => { setRegister(!register); setError(""); setPassword(""); setConfirmPassword(""); }}>{register ? t.accountAlreadyRegistered : t.accountRegister}</Button>
  </form>;
}
