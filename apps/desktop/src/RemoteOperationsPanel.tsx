import { ArrowRight, ArrowUpRight } from "lucide-react";
import { DirectoryBrowser } from "./DirectoryBrowser";
import { WindowBrowser } from "./WindowBrowser";
import { ScreenshotBrowser } from "./ScreenshotBrowser";
import { TerminalBrowser } from "./TerminalBrowser";
import { messages, type Language } from "./i18n";
import type { ConnectedDevice, TransferUpdate } from "./operatorTypes";

export type OperationKind = "command" | "transfer" | "directory" | "windows" | "screenshot" | "terminal";

type Props = {
  language: Language;
  selected: ConnectedDevice | undefined;
  operation: OperationKind;
  onOperationChange: (value: OperationKind) => void;
  program: string;
  onProgramChange: (value: string) => void;
  argumentsText: string;
  onArgumentsTextChange: (value: string) => void;
  cwd: string;
  onCwdChange: (value: string) => void;
  submitting: boolean;
  onRunCommand: () => void;
  transferDirection: "upload" | "download";
  onTransferDirectionChange: (value: "upload" | "download") => void;
  transferSource: string;
  onTransferSourceChange: (value: string) => void;
  transferDestination: string;
  onTransferDestinationChange: (value: string) => void;
  transferOverwrite: boolean;
  onTransferOverwriteChange: (value: boolean) => void;
  transfer: TransferUpdate | null;
  startingTransfer: boolean;
  onStartTransfer: () => void;
  onCancelTransfer: () => void;
  onAuditChange: () => void;
};

export function RemoteOperationsPanel({
  language, selected,
  operation, onOperationChange, program, onProgramChange, argumentsText,
  onArgumentsTextChange, cwd, onCwdChange, submitting, onRunCommand,
  transferDirection, onTransferDirectionChange, transferSource,
  onTransferSourceChange, transferDestination, onTransferDestinationChange,
  transferOverwrite, onTransferOverwriteChange, transfer, startingTransfer,
  onStartTransfer, onCancelTransfer, onAuditChange,
}: Props) {
  const t = messages[language];
  const operations: { kind: OperationKind; label: string }[] = [
    { kind: "command", label: t.commandTitle },
    { kind: "transfer", label: t.fileTransfer },
    { kind: "directory", label: t.directoryBrowse },
    ...(selected?.osFamily.toLowerCase().includes("windows")
      ? [{ kind: "windows" as const, label: t.windowList }]
      : []),
    { kind: "screenshot", label: t.screenshot },
    { kind: "terminal", label: t.terminal },
  ];

  return (
    <div className="remote-command device-command-content">
      {selected ? (
        <>
          <nav className="operation-list" role="tablist" aria-orientation="vertical" aria-label={t.commandTitle}>
            {operations.map(({ kind, label }) => (
              <button
                key={kind}
                role="tab"
                aria-selected={operation === kind}
                className={operation === kind ? "active" : ""}
                onClick={() => onOperationChange(kind)}
              >
                {label}
              </button>
            ))}
          </nav>
          <div className="operation-panel" role="tabpanel">
            {!selected.connected && <p className="form-hint">{t.reconnectHint}</p>}
            {operation === "command" && (
              <>
                <p className="form-hint">{t.nativeCommandHint}</p>
                <div className="command-fields">
                  <label>
                    <span className="field-label">{t.program}</span>
                    <input value={program} onChange={(event) => onProgramChange(event.target.value)} placeholder="powershell.exe / bash" />
                  </label>
                  <label>
                    <span className="field-label">{t.arguments}</span>
                    <textarea value={argumentsText} onChange={(event) => onArgumentsTextChange(event.target.value)} placeholder={t.argumentPlaceholder} />
                  </label>
                  <label>
                    <span className="field-label">{t.cwd}</span>
                    <input value={cwd} onChange={(event) => onCwdChange(event.target.value)} />
                  </label>
                </div>
                <button
                  className="primary-button"
                  disabled={submitting || !selected.connected || !program.trim()}
                  onClick={() => onRunCommand()}
                >
                  {submitting ? t.runningCommand : t.runCommand}
                  <ArrowRight size={17} />
                </button>
              </>
            )}
            {operation === "transfer" && (
              <>
                <div className="transfer-directions">
                  <button className={transferDirection === "upload" ? "active" : ""} onClick={() => onTransferDirectionChange("upload")}>{t.upload}</button>
                  <button className={transferDirection === "download" ? "active" : ""} onClick={() => onTransferDirectionChange("download")}>{t.download}</button>
                </div>
                <div className="command-fields">
                  <label><span className="field-label">{transferDirection === "upload" ? t.localSource : t.remoteSource}</span><input value={transferSource} onChange={(event) => onTransferSourceChange(event.target.value)} /></label>
                  <label><span className="field-label">{transferDirection === "upload" ? t.remoteDestination : t.localDestination}</span><input value={transferDestination} onChange={(event) => onTransferDestinationChange(event.target.value)} /></label>
                </div>
                <label className="check-row">
                  <input type="checkbox" checked={transferOverwrite} onChange={(event) => onTransferOverwriteChange(event.target.checked)} />
                  {t.overwriteExisting}
                </label>
                <button
                  className="primary-button"
                  disabled={startingTransfer || transfer?.state === "running" || !selected.connected || !transferSource.trim() || !transferDestination.trim()}
                  onClick={() => onStartTransfer()}
                >
                  {startingTransfer ? t.startingTransfer : t.startTransfer}
                  <ArrowRight size={17} />
                </button>
                {transfer && (
                  <div className="transfer-status">
                    <div><strong>{t.transferStates[transfer.state]}</strong><span>{transfer.size ? `${Math.round(transfer.offset / transfer.size * 100)}% · ${transfer.offset} / ${transfer.size} B` : ""}</span></div>
                    <progress value={transfer.offset} max={Math.max(transfer.size, 1)} />
                    {transfer.state === "cancel_requested" && <small>{t.transferCancelUnconfirmed}</small>}
                    {transfer.message && <small>{transfer.message}</small>}
                    {transfer.state === "running" && <button className="quiet-button" onClick={() => onCancelTransfer()}>{t.cancelTransfer}</button>}
                  </div>
                )}
              </>
            )}
            {operation === "directory" && <DirectoryBrowser code={selected.deviceCode} osFamily={selected.osFamily} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            {operation === "windows" && <WindowBrowser code={selected.deviceCode} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            {operation === "screenshot" && <ScreenshotBrowser code={selected.deviceCode} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            <TerminalBrowser code={selected.deviceCode} connected={selected.connected} language={language} visible={operation === "terminal"} onAuditChange={onAuditChange} />
          </div>
        </>
      ) : (
        <div className="empty-panel">
          <span><ArrowUpRight /></span>
          <strong>{t.selectDevice}</strong>
          <p>{t.selectDeviceHint}</p>
        </div>
      )}
    </div>
  );
}
