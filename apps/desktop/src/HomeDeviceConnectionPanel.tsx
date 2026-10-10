import type { FormEvent } from "react";
import { Button, Input } from "antd";
import { Plus } from "lucide-react";
import { formatDeviceCode, normalizeDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

type Props = {
  language: Language;
  signedIn: boolean;
  onSignIn: () => void;
  code: string;
  password: string;
  connecting: boolean;
  onCodeChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onConnect: () => void;
};

export function HomeDeviceConnectionPanel({
  language,
  signedIn,
  onSignIn,
  code,
  password,
  connecting,
  onCodeChange,
  onPasswordChange,
  onConnect,
}: Props) {
  const t = messages[language];

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!signedIn) { onSignIn(); return; }
    onConnect();
  }

  return (
    <section className="surface home-connect">
      <h2>{t.addDevice}</h2>
      <p>{signedIn ? t.addDeviceHint : t.addDeviceLoginRequired}</p>
      {!signedIn && <Button type="primary" onClick={onSignIn}>{t.addDeviceSignIn}</Button> }
      <form className="home-connect-form" onSubmit={submit}>
        <label>
          <span className="field-label">{t.remoteCode}</span>
          <Input
            value={formatDeviceCode(code)}
            onChange={(event) => onCodeChange(normalizeDeviceCode(event.target.value))}
            inputMode="numeric"
            maxLength={11}
            placeholder="000 000 000"
          />
        </label>
        <div className="home-connect-password-field">
          <label className="field-label" htmlFor="home-connect-password">{t.remotePassword}</label>
          <span className="home-account-password">
            <Input.Password
              id="home-connect-password"
              value={password}
              onChange={(event) => onPasswordChange(event.target.value)}
              placeholder="••••••••"
            />
          </span>
        </div>
        <Button type="primary" htmlType="submit" loading={connecting} disabled={!signedIn || code.length !== 9 || !password}
          icon={<Plus size={16} />} iconPlacement="end">{t.addDevice}</Button>
      </form>
    </section>
  );
}
