import { useEffect } from "react";
import { createPortal } from "react-dom";
import { Check, RotateCw, X } from "lucide-react";
import { formatDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

export type SavedConnection = {
  deviceId: string;
  deviceCode: string;
  name: string;
  phase: "preparing" | "waiting" | "connecting" | "retrying" | "checking" | "connected" | "failed";
  step: 0 | 1 | 2 | 3;
  message: string;
  attempt: number;
};

type Props = {
  language: Language;
  connection: SavedConnection;
  onClose: () => void;
  onRetry: () => void;
  onOpen: () => void;
};

export function SavedConnectionDialog({ language, connection, onClose, onRetry, onOpen }: Props) {
  const t = messages[language];
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [onClose]);

  const stage = {
    preparing: t.savedConnectPreparing,
    waiting: t.savedConnectWaiting,
    connecting: t.savedConnectConnecting,
    retrying: t.savedConnectRetrying,
    checking: t.savedConnectChecking,
    connected: t.savedConnectConnected,
    failed: t.savedConnectFailed,
  }[connection.phase];
  const steps = [t.savedConnectStepInfo, t.savedConnectStepNetwork, t.savedConnectStepVerify];
  const isPending = connection.phase !== "connected" && connection.phase !== "failed";

  return createPortal(
    <div className="device-dialog-backdrop" onMouseDown={onClose}>
      <div
        className="device-dialog saved-connect-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="saved-connect-title"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <div className="saved-connect-heading">
          <div>
            <h2 id="saved-connect-title">{t.savedConnectTitle}</h2>
            <p>{formatDeviceCode(connection.deviceCode)} · {connection.name}</p>
          </div>
          <button className="saved-connect-close" aria-label={t.savedConnectClose} autoFocus onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <div className={`saved-connect-stage ${connection.phase}`} role="status" aria-live="polite">
          {isPending && <span className="saved-connect-spinner" aria-hidden="true" />}
          {connection.phase === "connected" && <Check size={17} aria-hidden="true" />}
          {connection.phase === "failed" && <X size={17} aria-hidden="true" />}
          <span>{stage}</span>
        </div>
        <div className="saved-connect-progress-title">{t.savedConnectProgress}</div>
        <ol className="saved-connect-steps">
          {steps.map((label, index) => {
            const status = index < connection.step
              ? "done"
              : index === connection.step && connection.phase === "failed"
                ? "failed"
                : index === connection.step && isPending
                  ? "active"
                  : "pending";
            return (
              <li className={status} key={label}>
                <span className="saved-connect-step-icon">{status === "done" ? <Check size={13} /> : index + 1}</span>
                <span>{label}</span>
              </li>
            );
          })}
        </ol>
        {connection.phase === "failed" && connection.message && (
          <p className="device-dialog-error saved-connect-error" role="alert">{connection.message}</p>
        )}
        {isPending && <p className="saved-connect-note">{t.savedConnectBackground}</p>}
        <div className="device-dialog-actions">
          <button onClick={onClose}>{t.savedConnectClose}</button>
          {connection.phase === "failed" && (
            <button className="saved-connect-action" onClick={onRetry}>
              <RotateCw size={14} />{t.savedConnectRetry}
            </button>
          )}
          {connection.phase === "connected" && (
            <button className="saved-connect-action" onClick={onOpen}>{t.savedConnectOpen}</button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
