import { useState } from "react";
import { ArrowDown, ArrowUp, FileText, List, Monitor, RotateCw, Terminal } from "lucide-react";
import { Button, Pagination, Select, Tag } from "antd";
import { HistoryScreenshot } from "./HistoryScreenshot";
import { HistoryTerminal } from "./HistoryTerminal";
import { messages, type Language } from "./i18n";
import type { OperationEntry, TaskEntry } from "./operatorTypes";
import { formatDeviceCode } from "./deviceCode";

type Activity =
  | { kind: "command"; item: TaskEntry }
  | { kind: "operation"; item: OperationEntry };

type Props = {
  embedded?: boolean;
  language: Language;
  tasks: TaskEntry[];
  operations: OperationEntry[];
  totalCount: number;
  selectedId: string;
  onSelect: (id: string) => void;
  hasMore: boolean;
  hasMoreTasks: boolean;
  hasMoreOperations: boolean;
  loadingMore: boolean;
  refreshing: boolean;
  refreshError: string;
  onLoadMore: () => Promise<boolean>;
  deviceOptions?: { code: string; label: string }[];
  deviceCode?: string;
  onDeviceChange?: (code: string) => void;
  onRefresh: () => void;
  onRefreshTask: (id: string) => void;
};

export function ActivityPanel({
  embedded = false, language, tasks, operations, totalCount, selectedId, onSelect, hasMore,
  hasMoreTasks, hasMoreOperations, loadingMore, refreshing, refreshError, onLoadMore,
  deviceOptions, deviceCode = "", onDeviceChange, onRefresh, onRefreshTask,
}: Props) {
  const t = messages[language];
  const [page, setPage] = useState(1);
  const pageSize = 20;
  const activity: Activity[] = [
    ...tasks.map((item): Activity => ({ kind: "command", item })),
    ...operations.map((item): Activity => ({ kind: "operation", item })),
  ].sort((a, b) => b.item.startedAtUnixMs - a.item.startedAtUnixMs);
  const visible = activity.slice((page - 1) * pageSize, page * pageSize);
  const selected = visible.find(({ item }) => item.id === selectedId) ?? visible[0];
  const canGoNext = page * pageSize < activity.length || hasMore;
  async function nextPage() {
    const target = (page + 1) * pageSize;
    if ((hasMoreTasks && tasks.length < target) || (hasMoreOperations && operations.length < target)) {
      if (!await onLoadMore()) return;
    }
    setPage((current) => current + 1);
  }
  const selectedTask = selected?.kind === "command" ? selected.item : null;
  const selectedOperation = selected?.kind === "operation" ? selected.item : null;
  const formatTime = (value: number) => new Date(value).toLocaleString(language);
  const formatActor = (actor: string) => actor === "guest" ? t.guestOperator : actor;
  const isFileOperation = (kind: string) => Object.prototype.hasOwnProperty.call(t.fileOperations, kind);
  const isSystemOperation = (kind: string) => Object.prototype.hasOwnProperty.call(t.systemOperations, kind);
  const operationLabel = (item: OperationEntry) => isSystemOperation(item.kind) ? t.systemOperations[item.kind as keyof typeof t.systemOperations] : isFileOperation(item.kind)
    ? t.fileOperations[item.kind as keyof typeof t.fileOperations]
    : item.kind === "directory"
    ? t.directoryBrowse
    : item.kind === "windows"
    ? t.windowList
    : item.kind === "screenshot"
    ? t.screenshot
    : item.kind === "terminal"
    ? t.terminal
    : item.kind === "desktop_input"
    ? t.remoteInput
    : item.direction === "upload" ? t.upload : t.download;
  const operationStateLabel = (item: OperationEntry) => item.ui && item.state !== "running" && item.state !== "cancel_requested"
    ? t.uiControl.outcomes[item.ui.outcome]
    : item.kind === "terminal"
    ? t.terminalStates[item.state as keyof typeof t.terminalStates] ?? item.state
    : item.state === "cancel_requested"
    ? t.transferStates.cancel_requested
    : item.executionObservation === "unconfirmed"
    ? t.transferUnconfirmedShort
    : item.kind === "file_transfer" && item.state === "running" && item.phase
    ? t.transferPhases[item.phase as keyof typeof t.transferPhases] ?? t.transferStates.running
    : t.transferStates[item.state as keyof typeof t.transferStates] ?? item.state;
  const operationMessage = (item: OperationEntry) => {
    const code = item.ui?.errorCode;
    if (!code) return item.message;
    if (code.startsWith("ui_worker_") || code.startsWith("stale_element:")) return t.uiControl.errors.worker_reset;
    return t.uiControl.errors[code as keyof typeof t.uiControl.errors] ?? item.message;
  };

  return (
    <section className={`${embedded ? "device-activity-panel" : "surface"} activity-panel`}>
      <div className="surface-topline">
        <h2>{t.deviceTaskHistory}</h2>
        <div className="activity-actions">
          {onDeviceChange && <Select className="activity-device-select" aria-label={t.historyDeviceFilter} value={deviceCode}
            options={[{ value: "", label: t.historyAllDevices }, ...(deviceOptions ?? []).map((device) => ({ value: device.code, label: device.label }))]}
            onChange={(value) => { setPage(1); onDeviceChange(value); }} />}
          <Tag className="count-badge">{totalCount}</Tag>
          <Button type="text" className="activity-refresh" disabled={refreshing} onClick={() => { setPage(1); onRefresh(); }}>
            <RotateCw size={14} />
            {refreshing ? t.refreshingActivity : t.refreshActivity}
          </Button>
        </div>
      </div>
      {refreshError && <p className="inline-error" role="alert">{refreshError}</p>}
      {activity.length === 0 ? <div className="empty-panel"><span><List /></span><strong>{t.noTasks}</strong><p>{t.noTasksHint}</p></div> : (
        <div className="task-layout">
          <div className="task-list">
            {visible.map(({ kind, item }) => (
              <button className={`task-row ${selected?.item.id === item.id ? "active" : ""}`} key={item.id} onClick={() => onSelect(item.id)}>
                <span className="task-icon">{kind === "command" ? <Terminal size={18} /> : isSystemOperation(item.kind) ? <Monitor size={18} /> : isFileOperation(item.kind) ? <FileText size={18} /> : item.kind === "directory" || item.kind === "windows" || item.kind === "screenshot" || item.kind === "desktop_input" ? <Monitor size={18} /> : item.direction === "upload" ? <ArrowUp size={18} /> : <ArrowDown size={18} />}</span>
                <span><strong>{kind === "command" ? item.program : operationLabel(item)}</strong><small>{formatDeviceCode(item.deviceCode)} · {formatTime(item.startedAtUnixMs)}</small></span>
                <em>{kind === "command" ? item.state : operationStateLabel(item)}</em>
              </button>
            ))}
          </div>
          <div className="task-output" tabIndex={0}>
            {selectedTask && <>
              <div className="output-heading"><strong>{selectedTask.program}</strong><span>{selectedTask.state}</span></div>
              <div className="output-meta">{formatDeviceCode(selectedTask.deviceCode)} · {t.commandDirection} · {formatTime(selectedTask.startedAtUnixMs)}</div>
              <div className="command-audit"><span>{t.initiatedBy}</span><code>{formatActor(selectedTask.initiatedBy)}</code></div>
              <div className="command-audit"><span>{t.commandArguments}</span><code>{selectedTask.args.length ? selectedTask.args.join(" · ") : "—"}</code></div>
              {selectedTask.cwd && <div className="command-audit"><span>{t.cwd}</span><code>{selectedTask.cwd}</code></div>}
              <pre>{selectedTask.stdout || (!selectedTask.stderr && t.waitingOutput)}</pre>
              {selectedTask.stderr && <pre className="stderr-output">{selectedTask.stderr}</pre>}
              {!selectedTask.complete && <Button type="text" onClick={() => onRefreshTask(selectedTask.id)}>{t.loadMoreOutput}</Button>}
            </>}
            {selectedOperation && <>
              <div className="output-heading"><strong>{operationLabel(selectedOperation)}</strong><span>{operationStateLabel(selectedOperation)}</span></div>
              <div className="output-meta">{formatDeviceCode(selectedOperation.deviceCode)} · {formatTime(selectedOperation.startedAtUnixMs)}</div>
              {selectedOperation.kind === "screenshot" && selectedOperation.state === "completed" &&
                <HistoryScreenshot key={selectedOperation.id} id={selectedOperation.id} language={language} />}
              {selectedOperation.kind === "terminal" &&
                <HistoryTerminal key={selectedOperation.id} id={selectedOperation.id} offset={selectedOperation.offset} language={language} />}
              <div className="operation-details">
                {selectedOperation.ui && <>
                  <div><span>{t.uiControl.returned}</span><strong>{selectedOperation.ui.returnedCount}</strong></div>
                  <div><span>{t.uiControl.visited}</span><strong>{selectedOperation.ui.visitedCount}</strong></div>
                  {selectedOperation.kind === "ui_action" && <>
                    <div><span>{t.uiControl.dispatched}</span><strong>{selectedOperation.ui.actionDispatched === null ? t.uiControl.outcomes.unconfirmed : selectedOperation.ui.actionDispatched ? t.yes : t.no}</strong></div>
                    <div><span>{t.uiControl.verification}</span><strong>{t.uiControl.checks[selectedOperation.ui.verification]}</strong></div>
                  </>}
                  {selectedOperation.ui.truncated && <div><span>{t.result}</span><strong>{t.uiControl.truncated}</strong></div>}
                </>}
                {selectedOperation.executionObservation === "unconfirmed" && <div><span>{t.transferObservation}</span><strong>{selectedOperation.state === "cancel_requested" ? t.transferCancelUnconfirmed : t.transferUnconfirmed}</strong></div>}
                {selectedOperation.executionObservation === "unknown" && <div><span>{t.transferObservation}</span><strong>{t.transferUnknown}</strong></div>}
                <div><span>{t.initiatedBy}</span><strong>{formatActor(selectedOperation.initiatedBy)}</strong></div>
                <div><span>{t.direction}</span><strong>{isSystemOperation(selectedOperation.kind) ? t.systemOperations[selectedOperation.kind as keyof typeof t.systemOperations] : isFileOperation(selectedOperation.kind) ? t.fileOperations[selectedOperation.kind as keyof typeof t.fileOperations] : selectedOperation.kind === "terminal" ? t.terminalDirection : selectedOperation.kind === "desktop_input" ? t.remoteInputDirection : selectedOperation.kind === "windows" ? t.windowRead : selectedOperation.kind === "directory" ? t.directoryRead : selectedOperation.direction === "upload" ? t.uploadDirection : t.downloadDirection}</strong></div>
                {selectedOperation.kind === "desktop_input" && <div><span>{t.remoteInputType}</span><strong>{selectedOperation.source}</strong></div>}
                {["container", "container_logs", "container_control"].includes(selectedOperation.kind) && <div><span>{t.containerTarget}</span><strong>{selectedOperation.source}</strong></div>}
                {selectedOperation.kind.startsWith("git_") && <div><span>{t.gitRepository}</span><strong>{selectedOperation.source}</strong></div>}
                {isSystemOperation(selectedOperation.kind) && <div><span>{t.systemEntries}</span><strong>{selectedOperation.size.toLocaleString()}</strong></div>}
                {isFileOperation(selectedOperation.kind) && <>
                  <div><span>{t.sourcePath}</span><strong>{selectedOperation.source}</strong></div>
                  {selectedOperation.destination && <div><span>{t.destinationPath}</span><strong>{selectedOperation.destination}</strong></div>}
                  {selectedOperation.mutation && <>
                    <div><span>{t.mutationLabels.entries}</span><strong>{selectedOperation.mutation.processedEntries} / {selectedOperation.mutation.totalEntries}</strong></div>
                    <div><span>{t.mutationLabels.published}</span><strong>{selectedOperation.mutation.publishedEntries}</strong></div>
                    <div><span>{t.mutationLabels.deleted}</span><strong>{selectedOperation.mutation.deletedEntries}</strong></div>
                    {selectedOperation.mutation.partial && selectedOperation.state !== "running" && selectedOperation.state !== "cancel_requested" && <div><span>{t.result}</span><strong>{t.mutationLabels.partial}</strong></div>}
                    {selectedOperation.kind === "file_move" && <div><span>{t.mutationLabels.sourceRemoved}</span><strong>{selectedOperation.mutation.sourceRemoved ? t.yes : t.no}</strong></div>}
                  </>}
                  {["file_hash", "file_copy", "file_move", "file_delete", "archive_create", "archive_extract"].includes(selectedOperation.kind) ? <div><span>{t.progress}</span><strong>{selectedOperation.offset.toLocaleString()} / {selectedOperation.size.toLocaleString()} B</strong></div> : !["file_search", "mkdir"].includes(selectedOperation.kind) && <div><span>{t.imageSize}</span><strong>{selectedOperation.size.toLocaleString()} B</strong></div>}
                </>}
                {selectedOperation.kind === "directory" && <>
                  <div><span>{t.sourcePath}</span><strong>{selectedOperation.source}</strong></div>
                  <div><span>{t.directoryEntries}</span><strong>{selectedOperation.size.toLocaleString()}</strong></div>
                </>}
                {selectedOperation.kind === "windows" &&
                  <div><span>{t.windowCount}</span><strong>{selectedOperation.size.toLocaleString()}</strong></div>}
                {selectedOperation.kind === "screenshot" && <>
                  {selectedOperation.destination && <div><span>{t.destinationPath}</span><strong>{selectedOperation.destination}</strong></div>}
                  <div><span>{t.imageSize}</span><strong>{selectedOperation.size.toLocaleString()} B</strong></div>
                </>}
                {selectedOperation.kind === "file_transfer" && <>
                  <div><span>{t.sourcePath}</span><strong>{selectedOperation.source}</strong></div>
                  <div><span>{t.destinationPath}</span><strong>{selectedOperation.destination}</strong></div>
                  <div><span>{t.progress}</span><strong>{selectedOperation.offset.toLocaleString()} / {selectedOperation.size.toLocaleString()} B</strong></div>
                  <div><span>{t.overwrite}</span><strong>{selectedOperation.overwrite ? t.yes : t.no}</strong></div>
                </>}
                {selectedOperation.finishedAtUnixMs && <div><span>{t.finishedAt}</span><strong>{formatTime(selectedOperation.finishedAtUnixMs)}</strong></div>}
                {operationMessage(selectedOperation) && <div><span>{t.result}</span><strong>{operationMessage(selectedOperation)}</strong></div>}
              </div>
            </>}
          </div>
        </div>
      )}
      {totalCount > pageSize && <div className="activity-pagination">
        <Pagination simple={{ readOnly: true }} current={page} pageSize={pageSize} total={totalCount}
          showSizeChanger={false} disabled={loadingMore}
          onChange={(next) => {
            if (next === page + 1 && canGoNext) void nextPage();
            else if (next < page) setPage(next);
          }} />
      </div>}
    </section>
  );
}
