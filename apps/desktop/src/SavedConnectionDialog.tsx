import { Alert, Button, Modal, Spin, Steps } from "antd";
import { RotateCw } from "lucide-react";
import { formatDeviceCode } from "./deviceCode";
import { messages, type Language } from "./i18n";

export type SavedConnection = {
  mode: "manual" | "saved";
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

  return (
    <Modal open title={t.savedConnectTitle} width={430} className="saved-connect-dialog" maskClosable={false} keyboard={false}
      onCancel={onClose} footer={[
        <Button key="close" onClick={onClose}>{t.savedConnectClose}</Button>,
        connection.phase === "failed" && <Button key="retry" type="primary" icon={<RotateCw size={14} />} onClick={onRetry}>{t.savedConnectRetry}</Button>,
        connection.phase === "connected" && <Button key="open" type="primary" onClick={onOpen}>{t.savedConnectOpen}</Button>,
      ].filter(Boolean)}>
      <p className="saved-connect-target">{formatDeviceCode(connection.deviceCode)}{connection.name ? ` · ${connection.name}` : ""}</p>
      <div className={`saved-connect-stage ${connection.phase}`} role="status" aria-live="polite">
        {isPending && <Spin size="small" />}
        <span>{stage}</span>
      </div>
      <div className="saved-connect-progress-title">{t.savedConnectProgress}</div>
      <Steps direction="vertical" size="small" current={connection.step}
        status={connection.phase === "failed" ? "error" : "process"}
        items={steps.map((title) => ({ title }))} />
      {connection.phase === "failed" && connection.message &&
        <Alert className="saved-connect-error" type="error" showIcon message={connectionErrorMessage(connection.message, language)} />}
      {isPending && <p className="saved-connect-note">{t.savedConnectBackground}</p>}
    </Modal>
  );
}

function connectionErrorMessage(message: string, language: Language): string {
  const t = messages[language];
  const text = message.toLowerCase();
  if (text.includes("the device is offline")) return t.connectDeviceOffline;
  if (text.includes("the device rejected authentication")) return t.connectPasswordRejected;
  if (text.includes("permissiondenied") || text.includes("invalidcredentials")) return t.connectPermissionDenied;
  if (text.includes("ratelimited")) return t.connectRateLimited;
  if (text.includes("notfound")) return t.connectDeviceNotFound;
  if (text.includes("timed out")) return t.connectTimedOut;
  return message;
}
