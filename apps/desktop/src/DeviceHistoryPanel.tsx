import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ActivityPanel } from "./ActivityPanel";
import { messages, type Language } from "./i18n";
import type { HistoryPage, TaskEntry, TaskUpdate } from "./operatorTypes";

const emptyHistory: HistoryPage = {
  tasks: [],
  operations: [],
  totalCount: 0,
  taskBefore: null,
  operationBeforeStartedAtUnixMs: null,
  operationBeforeId: null,
  hasMoreTasks: false,
  hasMoreOperations: false,
};

type Props = {
  code: string;
  language: Language;
  embedded?: boolean;
  deviceOptions?: { code: string; label: string }[];
  onDeviceChange?: (code: string) => void;
};

export function DeviceHistoryPanel({ code, language, embedded = true, deviceOptions, onDeviceChange }: Props) {
  const t = messages[language];
  const [history, setHistory] = useState<HistoryPage>(emptyHistory);
  const [selectedId, setSelectedId] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState("");
  const refreshInFlight = useRef(false);

  async function refresh(reset: boolean) {
    if (refreshInFlight.current) return;
    refreshInFlight.current = true;
    if (reset) setRefreshing(true);
    try {
      const first = await invoke<HistoryPage>("operator_device_history_page", {
        code,
        taskBefore: null,
        operationBeforeStartedAtUnixMs: null,
        operationBeforeId: null,
        loadTasks: true,
        loadOperations: true,
      });
      setHistory((current) => {
        if (reset) return first;
        const newTasks = new Set(first.tasks.map((task) => task.id));
        const newOperations = new Set(first.operations.map((operation) => operation.id));
        const previousTasks = new Map(current.tasks.map((task) => [task.id, task]));
        return {
          ...current,
          totalCount: first.totalCount,
          tasks: [
            ...first.tasks.map((task) => {
              const old = previousTasks.get(task.id);
              if (!old) return task;
              return {
                ...task,
                complete: task.complete || old.complete,
                stdout: old.stdoutOffset > task.stdoutOffset ? old.stdout : task.stdout,
                stderr: old.stderrOffset > task.stderrOffset ? old.stderr : task.stderr,
                stdoutOffset: Math.max(old.stdoutOffset, task.stdoutOffset),
                stderrOffset: Math.max(old.stderrOffset, task.stderrOffset),
              };
            }),
            ...current.tasks.filter((task) => !newTasks.has(task.id)),
          ],
          operations: [
            ...first.operations,
            ...current.operations.filter((operation) => !newOperations.has(operation.id)),
          ],
          taskBefore: current.tasks.length ? current.taskBefore : first.taskBefore,
          operationBeforeStartedAtUnixMs:
            current.operations.length
              ? current.operationBeforeStartedAtUnixMs
              : first.operationBeforeStartedAtUnixMs,
          operationBeforeId: current.operations.length
            ? current.operationBeforeId
            : first.operationBeforeId,
          hasMoreTasks: current.tasks.length ? current.hasMoreTasks : first.hasMoreTasks,
          hasMoreOperations: current.operations.length
            ? current.hasMoreOperations
            : first.hasMoreOperations,
        };
      });
      setError("");
    } catch (cause) {
      setError(`${t.historyLoadFailed}: ${String(cause)}`);
    } finally {
      refreshInFlight.current = false;
      if (reset) setRefreshing(false);
    }
  }

  useEffect(() => {
    void refresh(true);
    const interval = window.setInterval(() => void refresh(false), 5000);
    return () => window.clearInterval(interval);
  }, [code, language]);

  async function loadMore() {
    if (loadingMore || (!history.hasMoreTasks && !history.hasMoreOperations)) return false;
    setLoadingMore(true);
    try {
      const next = await invoke<HistoryPage>("operator_device_history_page", {
        code,
        taskBefore: history.taskBefore,
        operationBeforeStartedAtUnixMs: history.operationBeforeStartedAtUnixMs,
        operationBeforeId: history.operationBeforeId,
        loadTasks: history.hasMoreTasks,
        loadOperations: history.hasMoreOperations,
      });
      setHistory((current) => {
        const knownTasks = new Set(current.tasks.map((task) => task.id));
        const knownOperations = new Set(current.operations.map((operation) => operation.id));
        return {
          tasks: [...current.tasks, ...next.tasks.filter((task) => !knownTasks.has(task.id))],
          totalCount: current.totalCount,
          operations: [
            ...current.operations,
            ...next.operations.filter((operation) => !knownOperations.has(operation.id)),
          ],
          taskBefore: history.hasMoreTasks ? next.taskBefore : current.taskBefore,
          operationBeforeStartedAtUnixMs: history.hasMoreOperations
            ? next.operationBeforeStartedAtUnixMs
            : current.operationBeforeStartedAtUnixMs,
          operationBeforeId: history.hasMoreOperations
            ? next.operationBeforeId
            : current.operationBeforeId,
          hasMoreTasks: history.hasMoreTasks ? next.hasMoreTasks : current.hasMoreTasks,
          hasMoreOperations: history.hasMoreOperations
            ? next.hasMoreOperations
            : current.hasMoreOperations,
        };
      });
      return true;
    } catch (cause) {
      setError(`${t.historyLoadFailed}: ${String(cause)}`);
      return false;
    } finally {
      setLoadingMore(false);
    }
  }

  async function refreshTask(taskId: string) {
    const task = history.tasks.find((item) => item.id === taskId);
    if (!task || task.complete) return;
    try {
      const update = await invoke<TaskUpdate>("operator_task", {
        taskId,
        stdoutOffset: task.stdoutOffset,
        stderrOffset: task.stderrOffset,
      });
      setHistory((current) => ({
        ...current,
        tasks: current.tasks.map((item): TaskEntry => item.id === taskId
          && item.stdoutOffset === task.stdoutOffset
          && item.stderrOffset === task.stderrOffset ? {
          ...item,
          state: update.state,
          complete: update.complete,
          stdout: item.stdout + update.stdout,
          stderr: item.stderr + update.stderr,
          stdoutOffset: update.stdoutOffset,
          stderrOffset: update.stderrOffset,
        } : item),
      }));
    } catch (cause) {
      setError(`${t.taskRefreshFailed}: ${String(cause)}`);
    }
  }

  return (
    <ActivityPanel
      embedded={embedded}
      language={language}
      tasks={history.tasks}
      operations={history.operations}
      totalCount={history.totalCount}
      selectedId={selectedId}
      onSelect={setSelectedId}
      hasMore={history.hasMoreTasks || history.hasMoreOperations}
      hasMoreTasks={history.hasMoreTasks}
      hasMoreOperations={history.hasMoreOperations}
      loadingMore={loadingMore}
      refreshing={refreshing}
      refreshError={error}
      onLoadMore={loadMore}
      deviceOptions={deviceOptions}
      deviceCode={code}
      onDeviceChange={onDeviceChange}
      onRefresh={() => void refresh(true)}
      onRefreshTask={(id) => void refreshTask(id)}
    />
  );
}
