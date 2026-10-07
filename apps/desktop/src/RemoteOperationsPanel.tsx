import { ArrowRight, ArrowUpRight } from "lucide-react";
import { useState } from "react";
import { Button, Checkbox, Input, Menu, Progress, Segmented } from "antd";
import { DirectoryBrowser } from "./DirectoryBrowser";
import { WindowBrowser } from "./WindowBrowser";
import { ScreenshotBrowser } from "./ScreenshotBrowser";
import { TerminalBrowser } from "./TerminalBrowser";
import { ExecutionIdentityView } from "./ExecutionIdentityView";
import { ApplicationBrowser } from "./ApplicationBrowser";
import { ExecutionPicker, selectedExecution, useExecutionContexts } from "./ExecutionPicker";
import type { ExecutionSelection } from "./executionQueries";
import { messages, type Language } from "./i18n";
import type { ConnectedDevice, TransferUpdate } from "./operatorTypes";

export type OperationKind = "command" | "transfer" | "directory" | "windows" | "screenshot" | "terminal" | "applications";

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
  onRunCommand: (execution: ExecutionSelection) => void;
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
  onStartTransfer: (execution: ExecutionSelection) => void;
  onInspectTransfer: () => void;
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
  onStartTransfer, onCancelTransfer, onInspectTransfer, onAuditChange,
}: Props) {
  const t = messages[language];
  const contexts = useExecutionContexts(selected?.deviceCode, !!selected?.connected);
  const [executionKey, setExecutionKey] = useState("service");
  const execution = selectedExecution(contexts.entries, executionKey, "user");
  const operations: { kind: OperationKind; label: string }[] = [
    { kind: "command", label: t.commandTitle },
    { kind: "transfer", label: t.fileTransfer },
    { kind: "directory", label: t.directoryBrowse },
    ...(/windows|mac/i.test(selected?.osFamily ?? "")
      ? [{ kind: "windows" as const, label: t.windowList }]
      : []),
    { kind: "screenshot", label: t.screenshot },
    { kind: "terminal", label: t.terminal },
    { kind: "applications", label: t.applications },
  ];

  return (
    <div className="remote-command device-command-content">
      {selected ? (
        <>
          <Menu className="operation-list" mode="inline" selectedKeys={[operation]}
            items={operations.map(({ kind, label }) => ({ key: kind, label }))}
            onClick={({ key }) => onOperationChange(key as OperationKind)} />
          <div className="operation-panel" role="tabpanel">
            {!selected.connected && <p className="form-hint">{t.reconnectHint}</p>}
            {["command", "directory", "terminal", "transfer"].includes(operation) && <ExecutionPicker language={language} contexts={contexts}
              mode="user" value={executionKey} onChange={setExecutionKey} connected={selected.connected} />}
            {operation === "command" && (
              <>
                <p className="form-hint">{t.nativeCommandHint}</p>
                <div className="command-fields">
                  <label>
                    <span className="field-label">{t.program}</span>
                    <Input value={program} onChange={(event) => onProgramChange(event.target.value)} placeholder="powershell.exe / bash" />
                  </label>
                  <label>
                    <span className="field-label">{t.arguments}</span>
                    <Input.TextArea value={argumentsText} onChange={(event) => onArgumentsTextChange(event.target.value)} placeholder={t.argumentPlaceholder} />
                  </label>
                  <label>
                    <span className="field-label">{t.cwd}</span>
                    <Input value={cwd} onChange={(event) => onCwdChange(event.target.value)} />
                  </label>
                </div>
                <Button type="primary"
                  className="primary-button"
                  loading={submitting} disabled={!selected.connected || !program.trim() || !execution}
                  onClick={() => { if (execution) onRunCommand(execution); }}
                >
                  {submitting ? t.runningCommand : t.runCommand}
                  <ArrowRight size={17} />
                </Button>
              </>
            )}
            {operation === "transfer" && (
              <>
                <Segmented className="transfer-directions" block value={transferDirection}
                  options={[{ value: "upload", label: t.upload }, { value: "download", label: t.download }]}
                  onChange={(value) => onTransferDirectionChange(value as "upload" | "download")} />
                <div className="command-fields">
                  <label><span className="field-label">{transferDirection === "upload" ? t.localSource : t.remoteSource}</span><Input value={transferSource} onChange={(event) => onTransferSourceChange(event.target.value)} /></label>
                  <label><span className="field-label">{transferDirection === "upload" ? t.remoteDestination : t.localDestination}</span><Input value={transferDestination} onChange={(event) => onTransferDestinationChange(event.target.value)} /></label>
                </div>
                <Checkbox className="check-row" checked={transferOverwrite} onChange={(event) => onTransferOverwriteChange(event.target.checked)}>{t.overwriteExisting}</Checkbox>
                <Button type="primary"
                  className="primary-button"
                  loading={startingTransfer} disabled={(!!transfer && ["running", "cancel_requested", "unconfirmed"].includes(transfer.state)) || !execution || !selected.connected || !transferSource.trim() || !transferDestination.trim()}
                  onClick={() => { if (execution) onStartTransfer(execution); }}
                >
                  {startingTransfer ? t.startingTransfer : t.startTransfer}
                  <ArrowRight size={17} />
                </Button>
                {transfer && (
                  <div className="transfer-status">
                    <div><strong>{t.transferStates[transfer.state]}</strong><span>{transfer.size ? `${Math.round(transfer.offset / transfer.size * 100)}% · ${transfer.offset} / ${transfer.size} B` : ""}</span></div>
                    <Progress percent={transfer.size ? Math.round(transfer.offset / transfer.size * 100) : 0} size="small" showInfo={false} />
                    {["cancel_requested", "unconfirmed"].includes(transfer.state) && <><small>{t.transferCancelUnconfirmed}</small><Button onClick={onInspectTransfer}>{t.appsInspect}</Button></>}
                    {transfer.executionIdentity && <ExecutionIdentityView identity={transfer.executionIdentity} language={language} />}
                    {transfer.message && <small>{transfer.message}</small>}
                    {transfer.state === "running" && <Button type="text" onClick={() => onCancelTransfer()}>{t.cancelTransfer}</Button>}
                  </div>
                )}
              </>
            )}
            {operation === "directory" && <DirectoryBrowser key={executionKey} execution={execution} code={selected.deviceCode} osFamily={selected.osFamily} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            {operation === "windows" && <WindowBrowser code={selected.deviceCode} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            {operation === "screenshot" && <ScreenshotBrowser code={selected.deviceCode} connected={selected.connected} language={language} onAuditChange={onAuditChange} />}
            <TerminalBrowser execution={execution} code={selected.deviceCode} connected={selected.connected} language={language} visible={operation === "terminal"} onAuditChange={onAuditChange} />
            <div hidden={operation !== "applications"}><ApplicationBrowser code={selected.deviceCode} connected={selected.connected} language={language} contexts={contexts} osFamily={selected.osFamily} onAuditChange={onAuditChange} /></div>
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
