import { userFacingError } from "./userFacingError";
import { useEffect, useRef, useState } from "react";
import { Alert, Button, Input, Modal, Spin, Steps } from "antd";
import { RotateCw } from "lucide-react";
import { formatDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

export type SavedConnectionAction = "connect" | "desktop" | "terminal-console" | "terminal-system" | "files";

export type SavedConnection = {
  mode: "manual" | "saved";
  action?: SavedConnectionAction;
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
  onRetry: (password?: string) => void;
  onOpen: () => void;
};

export function SavedConnectionDialog({ language, connection, onClose, onRetry, onOpen }: Props) {
  const t = messages[language];
  const [password, setPassword] = useState("");
  const submitted = useRef(false);
  useEffect(() => {
    setPassword("");
    submitted.current = false;
  }, [connection.attempt]);
  const passwordRequired = /device password has not been entered|the device rejected authentication|authenticationrejected|invalidcredentials|(?:incorrect|invalid|wrong) (?:device )?password/i.test(connection.message);
  function retry() {
    if (connection.phase !== "failed" || submitted.current || (passwordRequired && !password)) return;
    submitted.current = true;
    onRetry(password || undefined);
    setPassword("");
  }
  const adding = connection.mode === "manual";
  const stage = {
    preparing: t.savedConnectPreparing,
    waiting: t.savedConnectWaiting,
    connecting: t.savedConnectConnecting,
    retrying: t.savedConnectRetrying,
    checking: t.savedConnectChecking,
    connected: adding ? t.addDeviceSucceeded : t.savedConnectConnected,
    failed: adding ? t.addDeviceFailed : t.savedConnectFailed,
  }[connection.phase];
  const steps = [t.savedConnectStepInfo, t.savedConnectStepNetwork, t.savedConnectStepVerify];
  const isPending = connection.phase !== "connected" && connection.phase !== "failed";

  return (
    <Modal open title={adding ? t.addDevice : t.savedConnectTitle} width={430} className="saved-connect-dialog" maskClosable={false} keyboard={false}
      onCancel={onClose} footer={[
        <Button key="close" onClick={onClose}>{t.savedConnectClose}</Button>,
        connection.phase === "failed" && <Button key="retry" type="primary" icon={<RotateCw size={14} />} disabled={passwordRequired && !password} onClick={retry}>{t.savedConnectRetry}</Button>,
        connection.phase === "connected" && <Button key="open" type="primary" onClick={onOpen}>{t.savedConnectOpen}</Button>,
      ].filter(Boolean)}>
      <p className="saved-connect-target">{formatDeviceCode(connection.deviceCode)}{connection.name ? ` · ${connection.name}` : ""}</p>
      <div className={`saved-connect-stage ${connection.phase}`} role="status" aria-live="polite">
        {isPending && <Spin size="small" />}
        <span>{stage}</span>
      </div>
      <div className="saved-connect-progress-title">{adding ? t.addDeviceProgress : t.savedConnectProgress}</div>
      <Steps direction="vertical" size="small" current={connection.step}
        status={connection.phase === "failed" ? "error" : "process"}
        items={steps.map((title) => ({ title }))} />
      {connection.phase === "failed" && connection.message &&
        <Alert className="saved-connect-error" type="error" showIcon message={connectionErrorMessage(connection.message, language)} />}
      {connection.phase === "failed" && <div className="saved-connect-password">
        <label className="field-label" htmlFor="saved-connect-password">{t.remotePassword}</label>
        <Input.Password id="saved-connect-password" value={password} autoFocus={passwordRequired}
          autoComplete="off" onChange={(event) => setPassword(event.target.value)}
          onPressEnter={(event) => {
            event.preventDefault();
            event.stopPropagation();
            if (!event.nativeEvent.isComposing) retry();
          }} />
        <p className="saved-connect-note">{passwordRequired ? t.connectEnterPassword : t.connectReplacePassword}</p>
      </div>}
      {isPending && <p className="saved-connect-note">{adding ? t.addDeviceBackground : t.savedConnectBackground}</p>}
    </Modal>
  );
}

function connectionErrorMessage(message: string, language: Language): string {
  const t = messages[language];
  const text = message.toLowerCase();
  if (text.includes("device password has not been entered")) return t.connectEnterPassword;
  if (/device (?:is |was )?(?:offline|not online)|deviceoffline/.test(text)) return t.connectDeviceOffline;
  if (/the device rejected authentication|authenticationrejected|invalidcredentials|(?:incorrect|invalid|wrong) (?:device )?password/.test(text)) return t.connectPasswordRejected;
  if (text.includes("permissiondenied")) return t.connectPermissionDenied;
  if (text.includes("ratelimited")) return t.connectRateLimited;
  if (text.includes("notfound") || text.includes("saved device was not found")) return t.connectDeviceNotFound;
  if (/timed out|timeout/.test(text)) return t.connectTimedOut;
  if (/endpoint control supervisor is unavailable|endpoint control connection closed/.test(text)) return t.connectServiceUnavailable;
  return userFacingError(message, language);
}
