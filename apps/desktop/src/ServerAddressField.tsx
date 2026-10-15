import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button, Input } from "antd";
import { messages, type Language } from "./i18n";

type Props = {
  kind: "control" | "relay";
  language: Language;
  label: string;
  value: string;
  disabled: boolean;
  onChange: (value: string) => void;
};
type Result = { elapsedMs: number } | { error: string };

export function ServerAddressField({ kind, language, label, value, disabled, onChange }: Props) {
  const t = messages[language];
  const sequence = useRef(0);
  const [checking, setChecking] = useState(false);
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    sequence.current += 1;
    setChecking(false);
    setResult(null);
    return () => { sequence.current += 1; };
  }, [value]);

  async function verify() {
    const id = ++sequence.current;
    setChecking(true);
    setResult(null);
    try {
      const response = await invoke<{ elapsedMs: number }>("validate_operator_server", { kind, url: value.trim() });
      if (sequence.current === id) setResult(response);
    } catch (error) {
      if (sequence.current === id) setResult({ error: String(error) });
    } finally {
      if (sequence.current === id) setChecking(false);
    }
  }

  const failure = result && "error" in result;
  const errorText = failure ? t.settingsProbeErrors[result.error as keyof typeof t.settingsProbeErrors] ?? t.settingsProbeErrors.unreachable : "";
  return <div className="settings-server-field" data-server-kind={kind}>
    <label className="field-label" htmlFor={`server-${kind}`}>{label}</label>
    <div className="settings-server-input-row">
      <Input id={`server-${kind}`} disabled={disabled} placeholder={t.settingsServerPlaceholder}
        value={value} spellCheck={false} onChange={event => {
          sequence.current += 1;
          setResult(null);
          setChecking(false);
          onChange(event.target.value);
        }} />
      <Button disabled={disabled || !value.trim()} loading={checking} onClick={() => void verify()}>
        {checking ? t.settingsVerifying : t.settingsVerify}
      </Button>
    </div>
    <div className={`settings-server-result${failure ? " is-error" : ""}`} role="status" aria-live="polite">
      {result && (failure ? errorText : `${kind === "control" ? t.settingsControlVerified : t.settingsRelayVerified} (${result.elapsedMs} ms)`)}
    </div>
  </div>;
}
