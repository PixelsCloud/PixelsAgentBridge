import { useState } from "react";
import { ArrowDown, ArrowUp, List, Monitor, RotateCw, Terminal } from "lucide-react";
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
  embedded = false, language, tasks, operations, selectedId, onSelect, hasMore,
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
  const operationLabel = (item: OperationEntry) => item.kind === "directory"
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
  const operationStateLabel = (item: OperationEntry) => item.kind === "terminal"
    ? t.terminalStates[item.state as keyof typeof t.terminalStates] ?? item.state
    : item.state === "cancel_requested"
    ? t.transferStates.cancel_requested
    : item.executionObservation === "unconfirmed"
    ? t.transferUnconfirmedShort
    : t.transferStates[item.state as keyof typeof t.transferStates] ?? item.state;

  return (
    <section className={`${embedded ? "device-activity-panel" : "surface"} activity-panel`}>
      <div className="surface-topline">
        <h2>{t.deviceTaskHistory}</h2>
        <div className="activity-actions">
          {onDeviceChange && <select className="activity-device-select" aria-label={t.historyDeviceFilter} value={deviceCode} onChange={(event) => onDeviceChange(event.target.value)}>
            <option value="">{t.historyAllDevices}</option>
            {deviceOptions?.map((device) => <option key={device.code} value={device.code}>{device.label}</option>)}
          </select>}
          <span className="count-badge">{activity.length}{hasMore ? "+" : ""}</span>
          <button className="quiet-button activity-refresh" disabled={refreshing} onClick={() => { setPage(1); onRefresh(); }}>
            <RotateCw size={14} />
            {refreshing ? t.refreshingActivity : t.refreshActivity}
          </button>
        </div>
      </div>
      {refreshError && <p className="inline-error" role="alert">{refreshError}</p>}
      {activity.length === 0 ? <div className="empty-panel"><span><List /></span><strong>{t.noTasks}</strong><p>{t.noTasksHint}</p></div> : (
        <div className="task-layout">
          <div className="task-list">
            {visible.map(({ kind, item }) => (
              <button className={`task-row ${selected?.item.id === item.id ? "active" : ""}`} key={item.id} onClick={() => onSelect(item.id)}>
                <span className="task-icon">{kind === "command" ? <Terminal size={18} /> : item.kind === "directory" || item.kind === "windows" || item.kind === "screenshot" || item.kind === "desktop_input" ? <Monitor size={18} /> : item.direction === "upload" ? <ArrowUp size={18} /> : <ArrowDown size={18} />}</span>
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
              {!selectedTask.complete && <button className="quiet-button" onClick={() => onRefreshTask(selectedTask.id)}>{t.loadMoreOutput}</button>}
            </>}
            {selectedOperation && <>
              <div className="output-heading"><strong>{operationLabel(selectedOperation)}</strong><span>{operationStateLabel(selectedOperation)}</span></div>
              <div className="output-meta">{formatDeviceCode(selectedOperation.deviceCode)} · {formatTime(selectedOperation.startedAtUnixMs)}</div>
              {selectedOperation.kind === "screenshot" && selectedOperation.state === "completed" &&
                <HistoryScreenshot key={selectedOperation.id} id={selectedOperation.id} language={language} />}
              {selectedOperation.kind === "terminal" &&
                <HistoryTerminal key={selectedOperation.id} id={selectedOperation.id} offset={selectedOperation.offset} language={language} />}
              <div className="operation-details">
                {selectedOperation.executionObservation === "unconfirmed" && <div><span>{t.transferObservation}</span><strong>{selectedOperation.state === "cancel_requested" ? t.transferCancelUnconfirmed : t.transferUnconfirmed}</strong></div>}
                {selectedOperation.executionObservation === "unknown" && <div><span>{t.transferObservation}</span><strong>{t.transferUnknown}</strong></div>}
                <div><span>{t.initiatedBy}</span><strong>{formatActor(selectedOperation.initiatedBy)}</strong></div>
                <div><span>{t.direction}</span><strong>{selectedOperation.kind === "terminal" ? t.terminalDirection : selectedOperation.kind === "desktop_input" ? t.remoteInputDirection : selectedOperation.kind === "windows" ? t.windowRead : selectedOperation.kind === "directory" ? t.directoryRead : selectedOperation.direction === "upload" ? t.uploadDirection : t.downloadDirection}</strong></div>
                {selectedOperation.kind === "desktop_input" && <div><span>{t.remoteInputType}</span><strong>{selectedOperation.source}</strong></div>}
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
                {selectedOperation.message && <div><span>{t.result}</span><strong>{selectedOperation.message}</strong></div>}
              </div>
            </>}
          </div>
        </div>
      )}
      {activity.length > 0 && <div className="activity-pagination">
        <button className="quiet-button" disabled={page === 1 || loadingMore} onClick={() => setPage((current) => current - 1)}>{t.historyPreviousPage}</button>
        <span>{t.historyPageLabel.replace("{page}", String(page))}</span>
        <button className="quiet-button" disabled={!canGoNext || loadingMore} onClick={() => void nextPage()}>{loadingMore ? t.loadingHistory : t.historyNextPage}</button>
      </div>}
    </section>
  );
}
