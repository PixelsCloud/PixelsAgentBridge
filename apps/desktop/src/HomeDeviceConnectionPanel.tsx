import type { FormEvent } from "react";
import { ArrowRight } from "lucide-react";
import { formatDeviceCode, normalizeDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

type Props = {
  language: Language;
  code: string;
  password: string;
  connecting: boolean;
  error: string;
  onCodeChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onConnect: () => void;
};

export function HomeDeviceConnectionPanel({
  language,
  code,
  password,
  connecting,
  error,
  onCodeChange,
  onPasswordChange,
  onConnect,
}: Props) {
  const t = messages[language];

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    onConnect();
  }

  return (
    <section className="surface home-connect">
      <h2>{t.connect}</h2>
      <form className="home-connect-form" onSubmit={submit}>
        <label>
          <span className="field-label">{t.remoteCode}</span>
          <input
            value={formatDeviceCode(code)}
            onChange={(event) => onCodeChange(normalizeDeviceCode(event.target.value))}
            inputMode="numeric"
            maxLength={11}
            placeholder="000 000 000"
          />
        </label>
        <label>
          <span className="field-label">{t.remotePassword}</span>
          <input
            type="password"
            value={password}
            onChange={(event) => onPasswordChange(event.target.value)}
            placeholder="••••••••"
          />
        </label>
        <button className="primary-button" type="submit" disabled={connecting || code.length !== 9 || !password}>
          {connecting ? t.connecting : t.connect}<ArrowRight size={16} />
        </button>
      </form>
      {error && <div className="inline-error" role="alert">{error}</div>}
    </section>
  );
}
