import { useState, type FormEvent } from "react";
import { ArrowRight, Eye, EyeOff } from "lucide-react";
import { formatDeviceCode, normalizeDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

type Props = {
  language: Language;
  code: string;
  password: string;
  connecting: boolean;
  onCodeChange: (value: string) => void;
  onPasswordChange: (value: string) => void;
  onConnect: () => void;
};

export function HomeDeviceConnectionPanel({
  language,
  code,
  password,
  connecting,
  onCodeChange,
  onPasswordChange,
  onConnect,
}: Props) {
  const t = messages[language];
  const [showPassword, setShowPassword] = useState(false);

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
        <div className="home-connect-password-field">
          <label className="field-label" htmlFor="home-connect-password">{t.remotePassword}</label>
          <span className="home-account-password">
            <input
              id="home-connect-password"
              type={showPassword ? "text" : "password"}
              value={password}
              onChange={(event) => onPasswordChange(event.target.value)}
              placeholder="••••••••"
            />
            <button type="button" aria-label={showPassword ? t.hide : t.show}
              title={showPassword ? t.hide : t.show}
              onClick={() => setShowPassword((current) => !current)}>
              {showPassword ? <EyeOff size={16} /> : <Eye size={16} />}
            </button>
          </span>
        </div>
        <button className="primary-button" type="submit" disabled={connecting || code.length !== 9 || !password}>
          {connecting ? t.connecting : t.connect}<ArrowRight size={16} />
        </button>
      </form>
    </section>
  );
}
