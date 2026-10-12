import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { App as AntdApp, Button, Modal } from "antd";
import { Globe2 } from "lucide-react";
import { messages, type Language } from "./i18n";

type Handoff = { reason: string; expiresAtUnixMs: number; ready: boolean; cancelled: boolean };

export function BrowserHandoffNotice({ language }: { language: Language }) {
  const [handoff, setHandoff] = useState<Handoff | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const [cancelConfirmExpiresAt, setCancelConfirmExpiresAt] = useState<number | null>(null);
  const pending = useRef<Handoff | null>(null);
  const { message } = AntdApp.useApp();
  const t = messages[language];

  useEffect(() => {
    let active = true;
    const refresh = () => void invoke<Handoff | null>("browser_handoff_status")
      .then(value => {
        if (!active) return;
        if (!value && pending.current && Date.now() >= pending.current.expiresAtUnixMs) {
          void message.warning(t.browserHandoffTimedOut);
        }
        pending.current = value && !value.ready && !value.cancelled ? value : null;
        setHandoff(value);
        if (!value || value.ready || value.cancelled) setCancelConfirmExpiresAt(null);
        if (!value) setError(false);
      })
      .catch(() => { if (active) setHandoff(null); });
    refresh();
    const timer = window.setInterval(refresh, 3000);
    return () => { active = false; window.clearInterval(timer); };
  }, [language, message]);

  return <><Modal
    className="browser-handoff-modal"
    open={!!handoff && !handoff.ready && !handoff.cancelled}
    title={<span className="browser-handoff-title"><Globe2 size={20} /> {t.browserHandoffTitle}</span>}
    closable={false}
    maskClosable={false}
    keyboard={false}
    centered
    footer={<>
      <Button disabled={busy} onClick={() => setCancelConfirmExpiresAt(handoff?.expiresAtUnixMs ?? null)}>{t.browserHandoffCancel}</Button>
      <Button type="primary" loading={busy} onClick={() => {
      setBusy(true);
      setError(false);
      void invoke<Handoff>("browser_handoff_complete")
        .then(value => { pending.current = null; setHandoff(value); })
        .catch(() => setError(true))
        .finally(() => setBusy(false));
      }}>{t.browserHandoffDone}</Button>
    </>}
  >
    <p className="browser-handoff-reason">{handoff?.reason}</p>
    <p>{t.browserHandoffInstruction}</p>
    {error && <p className="browser-handoff-error" role="alert">{t.browserHandoffError}</p>}
  </Modal>
  <Modal
    className="browser-handoff-modal"
    open={cancelConfirmExpiresAt !== null && !!handoff && !handoff.ready && !handoff.cancelled}
    title={t.browserHandoffCancelConfirmTitle}
    centered
    zIndex={1100}
    closable={!busy}
    maskClosable={!busy}
    keyboard={!busy}
    onCancel={() => setCancelConfirmExpiresAt(null)}
    footer={<>
      <Button disabled={busy} onClick={() => setCancelConfirmExpiresAt(null)}>{t.browserHandoffKeepWaiting}</Button>
      <Button danger loading={busy} onClick={() => {
        if (handoff?.expiresAtUnixMs !== cancelConfirmExpiresAt) {
          setCancelConfirmExpiresAt(null);
          return;
        }
        setBusy(true);
        setError(false);
        void invoke<Handoff>("browser_handoff_cancel")
          .then(value => { pending.current = null; setHandoff(value); setCancelConfirmExpiresAt(null); })
          .catch(() => { setCancelConfirmExpiresAt(null); setError(true); })
          .finally(() => setBusy(false));
      }}>{t.browserHandoffConfirmCancel}</Button>
    </>}
  >
    <p>{t.browserHandoffCancelConfirmDescription}</p>
  </Modal></>;
}
