import { lazy, Suspense, useEffect, useRef, useState, type MouseEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowUpRight, Pencil, Trash2, Unplug } from "lucide-react";
import { messages, type Language } from "./i18n";
import type { ScopeStatus } from "./operatorTypes";
import type { View } from "./App";
import type { ConnectedDevice, HistoryPage, OperatorBootstrap, OperationEntry, TaskEntry, TaskUpdate, TransferUpdate } from "./operatorTypes";
import { RemoteConnectionPanel } from "./RemoteConnectionPanel";
import { DeviceDetailPanel } from "./DeviceDetailPanel";
import type { OperationKind } from "./RemoteOperationsPanel";
import { formatDeviceCode } from "./deviceCode";
import { AccountConnectionPanel } from "./AccountConnectionPanel";
import { HomeDeviceConnectionPanel } from "./HomeDeviceConnectionPanel";
import { OsLogo } from "./OsLogo";
import { SavedConnectionDialog, type SavedConnection } from "./SavedConnectionDialog";

const ActivityPanel = lazy(() => import("./ActivityPanel").then(({ ActivityPanel }) => ({ default: ActivityPanel })));
const DeviceHistoryPanel = lazy(() => import("./DeviceHistoryPanel").then(({ DeviceHistoryPanel }) => ({ default: DeviceHistoryPanel })));
const RemoteOperationsPanel = lazy(() => import("./RemoteOperationsPanel").then(({ RemoteOperationsPanel }) => ({ default: RemoteOperationsPanel })));

export function OperatorPanel({ language, view, onOpenRemote }: { language: Language; view: View; onOpenRemote: () => void }) {
  const t = messages[language];
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [devices, setDevices] = useState<ConnectedDevice[]>([]);
  const [devicesLoaded, setDevicesLoaded] = useState(false);
  const [devicePresence, setDevicePresence] = useState<Record<string, { name: string; online: boolean | null }>>({});
  const [contextMenu, setContextMenu] = useState<{ code: string; x: number; y: number } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ConnectedDevice | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [disconnectTarget, setDisconnectTarget] = useState<ConnectedDevice | null>(null);
  const [disconnecting, setDisconnecting] = useState(false);
  const [disconnectError, setDisconnectError] = useState("");
  const [renameTarget, setRenameTarget] = useState<ConnectedDevice | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [renaming, setRenaming] = useState(false);
  const [renameError, setRenameError] = useState("");
  const [selectedCode, setSelectedCode] = useState("");
  const [program, setProgram] = useState("");
  const [argumentsText, setArgumentsText] = useState("");
  const [cwd, setCwd] = useState("");
  const [tasks, setTasks] = useState<TaskEntry[]>([]);
  const [operations, setOperations] = useState<OperationEntry[]>([]);
  const [taskBefore, setTaskBefore] = useState<string | null>(null);
  const [operationBeforeStartedAtUnixMs, setOperationBeforeStartedAtUnixMs] = useState<number | null>(null);
  const [operationBeforeId, setOperationBeforeId] = useState<string | null>(null);
  const [hasMoreTasks, setHasMoreTasks] = useState(false);
  const [hasMoreOperations, setHasMoreOperations] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [refreshingHistory, setRefreshingHistory] = useState(false);
  const [historyError, setHistoryError] = useState("");
  const [selectedActivityId, setSelectedActivityId] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState("");
  const [savedConnection, setSavedConnection] = useState<SavedConnection | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const [activeScope, setActiveScope] = useState<ScopeStatus | null>(null);
  const [aliasDraft, setAliasDraft] = useState("");
  const [operation, setOperation] = useState<OperationKind>("command");
  const [transferDirection, setTransferDirection] = useState<"upload" | "download">("upload");
  const [transferSource, setTransferSource] = useState("");
  const [transferDestination, setTransferDestination] = useState("");
  const [transferOverwrite, setTransferOverwrite] = useState(false);
  const [transfer, setTransfer] = useState<TransferUpdate | null>(null);
  const [startingTransfer, setStartingTransfer] = useState(false);
  const tasksRef = useRef(tasks);
  const savedConnectionAttemptRef = useRef(0);
  const historyRefreshRef = useRef<Promise<void> | null>(null);
  const refreshingRef = useRef(new Set<string>());
  const refreshAgainRef = useRef(new Set<string>());
  const selected = devices.find((device) => device.deviceCode === selectedCode);
  const menuDevice = devices.find((device) => device.deviceCode === contextMenu?.code);
  const connectedCodes = devices.filter((device) => device.connected).map((device) => device.deviceCode).join(",");
  const savedCodes = devices.map((device) => device.deviceCode).join(",");

  useEffect(() => {
    if (view !== "remote") return;
    if (devices.some((device) => device.deviceCode === selectedCode)) return;
    setSelectedCode(devices.find((device) => device.connected)?.deviceCode ?? devices[0]?.deviceCode ?? "");
  }, [view, devices, selectedCode]);

  useEffect(() => {
    if ((view !== "home" && view !== "remote") || !savedCodes) return;
    let closed = false;
    let refreshing = false;
    const codes = savedCodes.split(",");
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const statuses = await invoke<{ deviceCode: string; name: string; online: boolean | null }[]>(
          "operator_saved_device_presence",
        );
        if (closed) return;
        setDevicePresence((current) => {
          const next = { ...current };
          for (const status of statuses) {
            next[status.deviceCode] = {
              name: status.name || current[status.deviceCode]?.name || "",
              online: status.online,
            };
          }
          return next;
        });
      } catch {
        if (!closed) {
          setDevicePresence((current) => {
            const next = { ...current };
            for (const code of codes) {
              next[code] = { name: current[code]?.name || "", online: null };
            }
            return next;
          });
        }
      } finally {
        refreshing = false;
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), 15000);
    return () => {
      closed = true;
      window.clearInterval(interval);
    };
  }, [view, savedCodes]);

  useEffect(() => {
    if ((view !== "home" && view !== "remote") || !savedCodes) return;
    let closed = false;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const statuses = await invoke<{ deviceCode: string; path: ConnectedDevice["connectionPath"] }[]>(
          "operator_connection_paths",
        );
        if (closed) return;
        const paths = new Map(statuses.map((status) => [status.deviceCode, status.path]));
        setDevices((current) => current.map((device) => {
          if (!paths.has(device.deviceCode)) return device;
          const connectionPath = paths.get(device.deviceCode) ?? null;
          const connected = connectionPath !== null;
          return device.connected === connected && device.connectionPath === connectionPath
            ? device
            : { ...device, connected, connectionPath };
        }));
      } catch {
        if (!closed) {
          setDevices((current) => current.map((device) =>
            device.connectionPath ? { ...device, connectionPath: null } : device));
        }
      } finally {
        refreshing = false;
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), 1000);
    return () => {
      closed = true;
      window.clearInterval(interval);
    };
  }, [view, savedCodes]);

  function handleScopeChange(scope: ScopeStatus | null) {
    setActiveScope(scope);
    setDevices((current) => current.map((device) => ({ ...device, connected: false })));
    setSelectedCode("");
  }

  useEffect(() => {
    if (!contextMenu) return;
    const closeOnPointer = (event: PointerEvent) => {
      if (!(event.target as Element).closest(".device-context-menu")) setContextMenu(null);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setContextMenu(null);
    };
    const closeOnScroll = () => setContextMenu(null);
    window.addEventListener("pointerdown", closeOnPointer);
    window.addEventListener("keydown", closeOnEscape);
    window.addEventListener("scroll", closeOnScroll, true);
    return () => {
      window.removeEventListener("pointerdown", closeOnPointer);
      window.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("scroll", closeOnScroll, true);
    };
  }, [contextMenu]);

  useEffect(() => {
    if (!deleteTarget) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !deleting) setDeleteTarget(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [deleteTarget, deleting]);

  function openDeviceMenu(event: MouseEvent<HTMLButtonElement>, device: ConnectedDevice) {
    event.preventDefault();
    setContextMenu({
      code: device.deviceCode,
      x: Math.min(event.clientX, window.innerWidth - 188),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - 184)),
    });
  }

  async function connectSaved(device: ConnectedDevice) {
    setContextMenu(null);
    const attempt = ++savedConnectionAttemptRef.current;
    setSavedConnection({
      deviceId: device.deviceId,
      deviceCode: device.deviceCode,
      name: device.alias || devicePresence[device.deviceCode]?.name || t.unnamedDevice,
      phase: device.connected ? "connected" : "preparing",
      step: device.connected ? 3 : 0,
      message: "",
      attempt,
    });
    if (device.connected) {
      setSelectedCode(device.deviceCode);
      setAliasDraft(device.alias);
      return;
    }
    try {
      const connected = await invoke<ConnectedDevice>("operator_connect_saved", { code: device.deviceCode });
      setDevices((current) => [connected, ...current.filter((item) => item.deviceCode !== connected.deviceCode)]);
      setSelectedCode(connected.deviceCode);
      setAliasDraft(connected.alias);
      setSavedConnection((current) => current?.attempt === attempt
        ? { ...current, phase: "connected", step: 3, message: "" }
        : current);
    } catch (cause) {
      if (savedConnectionAttemptRef.current === attempt) {
        setDevices((current) => current.map((item) => item.deviceId === device.deviceId
          ? { ...item, connected: false }
          : item));
      }
      setSavedConnection((current) => current?.attempt === attempt
        ? { ...current, phase: "failed", message: String(cause) }
        : current);
    }
  }

  async function forgetDevice() {
    if (!deleteTarget || deleting) return;
    setDeleting(true);
    setDeleteError("");
    try {
      await invoke("operator_forget_device", { code: deleteTarget.deviceCode });
      setDevices((current) => current.filter((device) => device.deviceCode !== deleteTarget.deviceCode));
      if (selectedCode === deleteTarget.deviceCode) {
        setSelectedCode("");
        setAliasDraft("");
      }
      if (code === deleteTarget.deviceCode) setCode("");
      setDeleteTarget(null);
    } catch (cause) {
      setDeleteError(`${t.deleteDeviceFailed}: ${String(cause)}`);
    } finally {
      setDeleting(false);
    }
  }

  async function disconnectDevice() {
    if (!disconnectTarget || disconnecting) return;
    setDisconnecting(true);
    setDisconnectError("");
    try {
      await invoke("operator_disconnect_device", { code: disconnectTarget.deviceCode });
      setDevices((current) => current.map((device) => device.deviceId === disconnectTarget.deviceId
        ? { ...device, connected: false }
        : device));
      setDisconnectTarget(null);
    } catch (cause) {
      setDisconnectError(`${t.disconnectFailed}: ${String(cause)}`);
    } finally {
      setDisconnecting(false);
    }
  }

  useEffect(() => {
    void invoke<ScopeStatus | null>("operator_current_traffic_scope")
      .then(setActiveScope)
      .catch(() => setActiveScope(null));
  }, []);

  async function refreshOperations() {
    const saved = await invoke<OperationEntry[]>("operator_operations");
    setTransfer((current) => {
      if (!current) return current;
      const record = saved.find((item) => item.id === current.id);
      if (!record || !["running", "cancel_requested", "completed", "failed"].includes(record.state)) {
        return current;
      }
      return {
        ...current,
        state: record.state as TransferUpdate["state"],
        offset: record.offset,
        size: record.size,
        message: record.message,
      };
    });
    setOperations((current) => {
      const refreshed = new Set(saved.map((item) => item.id));
      return [...saved, ...current.filter((item) => !refreshed.has(item.id))];
    });
  }

  function refreshActivity(): Promise<void> {
    if (historyRefreshRef.current) return historyRefreshRef.current;
    const refresh = invoke<HistoryPage>("operator_history_page", {
      taskBefore: null,
      operationBeforeStartedAtUnixMs: null,
      operationBeforeId: null,
      loadTasks: true,
      loadOperations: true,
    }).then((page) => {
      setTasks((current) => {
        const previous = new Map(current.map((task) => [task.id, task]));
        const fresh = new Set(page.tasks.map((task) => task.id));
        const updated = page.tasks.map((task) => {
          const old = previous.get(task.id);
          if (!old) return task;
          return {
            ...task,
            complete: task.complete || old.complete,
            stdout: old.stdoutOffset > task.stdoutOffset ? old.stdout : task.stdout,
            stderr: old.stderrOffset > task.stderrOffset ? old.stderr : task.stderr,
            stdoutOffset: Math.max(old.stdoutOffset, task.stdoutOffset),
            stderrOffset: Math.max(old.stderrOffset, task.stderrOffset),
          };
        });
        const next = [...updated, ...current.filter((task) => !fresh.has(task.id))];
        tasksRef.current = next;
        return next;
      });
      setOperations((current) => {
        const fresh = new Set(page.operations.map((operation) => operation.id));
        return [...page.operations, ...current.filter((operation) => !fresh.has(operation.id))];
      });
      const latest = [...page.tasks, ...page.operations]
        .sort((first, second) => second.startedAtUnixMs - first.startedAtUnixMs)[0];
      setSelectedActivityId((current) => current || latest?.id || "");
      setHistoryError("");
    }).finally(() => {
      historyRefreshRef.current = null;
    });
    historyRefreshRef.current = refresh;
    return refresh;
  }

  async function refreshActivityManually() {
    setRefreshingHistory(true);
    try {
      await refreshActivity();
    } catch {
      setHistoryError(t.historyLoadFailed);
    } finally {
      setRefreshingHistory(false);
    }
  }

  async function loadMoreHistory() {
    if (loadingHistory || (!hasMoreTasks && !hasMoreOperations)) return;
    setLoadingHistory(true);
    try {
      const page = await invoke<HistoryPage>("operator_history_page", {
        taskBefore,
        operationBeforeStartedAtUnixMs,
        operationBeforeId,
        loadTasks: hasMoreTasks,
        loadOperations: hasMoreOperations,
      });
      setTasks((current) => {
        const known = new Set(current.map((item) => item.id));
        const next = [...current, ...page.tasks.filter((item) => !known.has(item.id))];
        tasksRef.current = next;
        return next;
      });
      setOperations((current) => {
        const known = new Set(current.map((item) => item.id));
        return [...current, ...page.operations.filter((item) => !known.has(item.id))];
      });
      if (hasMoreTasks) {
        setTaskBefore(page.taskBefore);
        setHasMoreTasks(page.hasMoreTasks);
      }
      if (hasMoreOperations) {
        setOperationBeforeStartedAtUnixMs(page.operationBeforeStartedAtUnixMs);
        setOperationBeforeId(page.operationBeforeId);
        setHasMoreOperations(page.hasMoreOperations);
      }
    } catch {
      setHistoryError(t.historyLoadFailed);
    } finally {
      setLoadingHistory(false);
    }
  }

  useEffect(() => {
    tasksRef.current = tasks;
  }, [tasks]);

  async function refreshTask(taskId: string) {
    const task = tasksRef.current.find((item) => item.id === taskId);
    if (!task || task.complete) return;
    if (refreshingRef.current.has(taskId)) {
      refreshAgainRef.current.add(taskId);
      return;
    }
    refreshingRef.current.add(taskId);
    try {
      const update = await invoke<TaskUpdate>("operator_task", {
        taskId,
        stdoutOffset: task.stdoutOffset,
        stderrOffset: task.stderrOffset,
      });
      setTasks((current) => {
        const next = current.map((item) => {
          if (item.id !== taskId || item.stdoutOffset !== task.stdoutOffset || item.stderrOffset !== task.stderrOffset) {
            return item;
          }
          return {
            ...item,
            state: update.state,
            complete: update.complete,
            stdout: item.stdout + update.stdout,
            stderr: item.stderr + update.stderr,
            stdoutOffset: update.stdoutOffset,
            stderrOffset: update.stderrOffset,
          };
        });
        tasksRef.current = next;
        return next;
      });
    } catch (error) {
      setHistoryError(`${t.taskRefreshFailed}: ${String(error)}`);
    } finally {
      refreshingRef.current.delete(taskId);
      if (refreshAgainRef.current.delete(taskId)) {
        void refreshTask(taskId);
      }
    }
  }

  useEffect(() => {
    let closed = false;
    let stopTask: (() => void) | undefined;
    let stopDevice: (() => void) | undefined;
    let stopTransfer: (() => void) | undefined;
    let bootstrapRetry: number | undefined;
    const loadBootstrap = async () => {
      try {
        const saved = await invoke<OperatorBootstrap>("operator_bootstrap");
        if (closed) return;
        setDevices((current) => {
          const known = new Set(current.map((device) => device.deviceId));
          return [...current, ...saved.devices.filter((device) => !known.has(device.deviceId))];
        });
        setDevicesLoaded(true);
        setTasks((current) => {
          const known = new Set(current.map((item) => item.id));
          const next = [...current, ...saved.tasks.filter((item) => !known.has(item.id))];
          tasksRef.current = next;
          return next;
        });
        setOperations((current) => {
          const known = new Set(current.map((item) => item.id));
          return [...current, ...saved.operations.filter((item) => !known.has(item.id))];
        });
        setTaskBefore(saved.taskBefore);
        setOperationBeforeStartedAtUnixMs(saved.operationBeforeStartedAtUnixMs);
        setOperationBeforeId(saved.operationBeforeId);
        setHasMoreTasks(saved.hasMoreTasks);
        setHasMoreOperations(saved.hasMoreOperations);
        const latest = [...saved.tasks, ...saved.operations]
          .sort((a, b) => b.startedAtUnixMs - a.startedAtUnixMs)[0];
        setSelectedActivityId((current) => current || latest?.id || "");
        setError((current) => current === t.bootstrapRetrying ? "" : current);
      } catch {
        if (closed) return;
        setError((current) => current || t.bootstrapRetrying);
        bootstrapRetry = window.setTimeout(() => void loadBootstrap(), 3000);
      }
    };
    void (async () => {
      stopTask = await listen<{ taskId: string }>("operator-task-changed", (event) => {
        void refreshTask(event.payload.taskId);
      });
      if (closed) {
        stopTask();
        return;
      }
      stopDevice = await listen<{ deviceId: string; phase: string }>("operator-device-connection", (event) => {
        setDevices((current) => current.map((device) => device.deviceId === event.payload.deviceId ? {
          ...device,
          connected: event.payload.phase === "connected",
          connectionPath: null,
        } : device));
        setSavedConnection((current) => {
          if (!current || current.deviceId !== event.payload.deviceId || current.phase === "connected" || current.phase === "failed") {
            return current;
          }
          const phase = event.payload.phase === "waitingforbridge"
            ? "waiting"
            : event.payload.phase === "connected"
              ? "checking"
              : event.payload.phase;
          return phase === "connecting" || phase === "retrying" || phase === "waiting" || phase === "checking"
            ? { ...current, phase, step: phase === "checking" ? 2 : 1 }
            : current;
        });
      });
      if (closed) {
        stopDevice();
        return;
      }
      stopTransfer = await listen<TransferUpdate>("operator-transfer", (event) => {
        setTransfer((current) => {
          const next = event.payload;
          if (current && current.id !== next.id) return current;
          if (next.state !== "running" && current?.id === next.id) {
            return { ...next, offset: current.offset, size: current.size };
          }
          return next;
        });
        setOperations((current) => current.map((item) => item.id === event.payload.id ? {
          ...item,
          state: event.payload.state,
          offset: event.payload.state === "running" ? event.payload.offset : item.offset,
          size: event.payload.state === "running" ? event.payload.size : item.size,
          message: event.payload.message,
          executionObservation: event.payload.state === "cancel_requested" ? "unconfirmed" : event.payload.state === "running" ? item.executionObservation : null,
          finishedAtUnixMs: event.payload.state === "running" || event.payload.state === "cancel_requested" ? null : Date.now(),
        } : item));
        if (event.payload.state !== "running") {
          void refreshOperations();
        }
      });
      if (closed) {
        stopTransfer();
        return;
      }
      await loadBootstrap();
    })();
    return () => {
      closed = true;
      if (bootstrapRetry !== undefined) window.clearTimeout(bootstrapRetry);
      stopTask?.();
      stopDevice?.();
      stopTransfer?.();
    };
  }, []);

  useEffect(() => {
    setAliasDraft(selected?.alias ?? "");
  }, [selectedCode, selected?.alias]);

  useEffect(() => {
    if (!connectedCodes) return;
    let closed = false;
    let refreshing = false;
    const refreshPresence = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        const codes = connectedCodes.split(",");
        const results = await Promise.allSettled(codes.map((code) => invoke<number>("operator_presence", { code })));
        if (closed) return;
        const counts = new Map<string, number>();
        results.forEach((result, index) => {
          if (result.status === "fulfilled") counts.set(codes[index], result.value);
        });
        setDevices((current) => current.map((device) => {
          const count = counts.get(device.deviceCode);
          return count === undefined || count === device.activeOperators ? device : { ...device, activeOperators: count };
        }));
      } finally {
        refreshing = false;
      }
    };
    void refreshPresence();
    const interval = window.setInterval(() => void refreshPresence(), 10000);
    return () => {
      closed = true;
      window.clearInterval(interval);
    };
  }, [connectedCodes]);

  useEffect(() => {
    if (operation === "windows" && selected && !selected.osFamily.toLowerCase().includes("windows")) {
      setOperation("command");
    }
  }, [operation, selected?.osFamily]);

  useEffect(() => {
    if (view !== "activity" && transfer?.state !== "running" && transfer?.state !== "cancel_requested") return;
    const refresh = () => {
      if (view === "activity") {
        void refreshActivity().catch(() => setHistoryError(t.historyLoadFailed));
      } else {
        void refreshOperations().catch(() => setError(t.historyLoadFailed));
      }
    };
    refresh();
    const interval = window.setInterval(() => {
      refresh();
    }, 2000);
    return () => window.clearInterval(interval);
  }, [view, language, transfer?.state]);

  useEffect(() => {
    if (view !== "activity") return;
    const interval = window.setInterval(() => {
      const connectedCodes = new Set(devices.filter((device) => device.connected).map((device) => device.deviceCode));
      for (const task of tasksRef.current.filter((item) => !item.complete && connectedCodes.has(item.deviceCode)).slice(0, 8)) {
        void refreshTask(task.id);
      }
    }, 3000);
    return () => window.clearInterval(interval);
  }, [view, devices]);

  async function connect() {
    if (connecting || code.length !== 9 || !password) return;
    setConnecting(true);
    setConnectError("");
    try {
      const device = await invoke<ConnectedDevice>("operator_connect", {
        code: code.trim(),
        password,
      });
      setDevices((current) => [device, ...current.filter((item) => item.deviceCode !== device.deviceCode)]);
      setSelectedCode(device.deviceCode);
      setCode("");
      setPassword("");
      setAliasDraft(device.alias);
      onOpenRemote();
    } catch (error) {
      setConnectError(`${t.connectFailed}: ${String(error)}`);
    } finally {
      setConnecting(false);
    }
  }

  async function runCommand() {
    if (!selected?.connected || !program.trim()) return;
    setSubmitting(true);
    setError("");
    try {
      const id = await invoke<string>("operator_run_command", {
        code: selected.deviceCode,
        program: program.trim(),
        args: argumentsText.split(/\r?\n/).filter((line) => line.length > 0),
        cwd: cwd.trim() || null,
      });
      const newTask: TaskEntry = {
        id,
        deviceCode: selected.deviceCode,
        initiatedBy: "guest",
        program: program.trim(),
        args: argumentsText.split(/\r?\n/).filter((line) => line.length > 0),
        cwd: cwd.trim() || null,
        startedAtUnixMs: Date.now(),
        state: "Accepted",
        complete: false,
        stdout: "",
        stderr: "",
        stdoutOffset: 0,
        stderrOffset: 0,
      };
      tasksRef.current = [newTask, ...tasksRef.current];
      setTasks(tasksRef.current);
      setSelectedActivityId(id);
      void refreshTask(id);
    } catch {
      setError(t.commandFailed);
    } finally {
      setSubmitting(false);
    }
  }

  async function renameDevice() {
    if (!selected) return;
    try {
      const alias = await invoke<string>("operator_rename_device", {
        code: selected.deviceCode,
        alias: aliasDraft,
      });
      setDevices((current) => current.map((device) => device.deviceCode === selected.deviceCode ? {
        ...device,
        alias,
      } : device));
      setError("");
    } catch {
      setError(t.renameFailed);
    }
  }

  async function renameSavedDevice() {
    if (!renameTarget || renaming) return;
    setRenaming(true);
    setRenameError("");
    try {
      const alias = await invoke<string>("operator_rename_device", {
        code: renameTarget.deviceCode,
        alias: renameDraft,
      });
      setDevices((current) => current.map((device) =>
        device.deviceCode === renameTarget.deviceCode ? { ...device, alias } : device));
      if (selectedCode === renameTarget.deviceCode) setAliasDraft(alias);
      setRenameTarget(null);
    } catch (cause) {
      setRenameError(`${t.renameFailed}: ${String(cause)}`);
    } finally {
      setRenaming(false);
    }
  }

  async function startTransfer() {
    if (!selected?.connected || !transferSource.trim() || !transferDestination.trim()) return;
    setStartingTransfer(true);
    setError("");
    try {
      const id = await invoke<string>("operator_start_transfer", {
        code: selected.deviceCode,
        direction: transferDirection,
        source: transferSource.trim(),
        destination: transferDestination.trim(),
        overwrite: transferOverwrite,
      });
      setTransfer((current) => current?.id === id ? current : {
        id,
        state: "running",
        offset: 0,
        size: 0,
        message: null,
      });
      setOperations((current) => [{
        id,
        deviceCode: selected.deviceCode,
        initiatedBy: "guest",
        kind: "file_transfer",
        direction: transferDirection,
        source: transferSource.trim(),
        destination: transferDestination.trim(),
        overwrite: transferOverwrite,
        state: "running",
        offset: 0,
        size: 0,
        startedAtUnixMs: Date.now(),
        finishedAtUnixMs: null,
        message: null,
        executionObservation: "active",
      }, ...current.filter((item) => item.id !== id)]);
      setSelectedActivityId(id);
    } catch (cause) {
      setError(`${t.transferFailed}: ${String(cause)}`);
    } finally {
      setStartingTransfer(false);
    }
  }

  async function cancelTransfer() {
    if (!transfer || transfer.state !== "running") return;
    try {
      await invoke("operator_cancel_transfer", { id: transfer.id });
    } catch (cause) {
      setError(String(cause));
    }
  }

  return (
    <>
      {view === "home" && (
        <HomeDeviceConnectionPanel
          language={language}
          code={code}
          password={password}
          connecting={connecting}
          error={connectError}
          onCodeChange={setCode}
          onPasswordChange={setPassword}
          onConnect={() => void connect()}
        />
      )}
      {view === "me" && (
        <AccountConnectionPanel
          language={language}
          activeScope={activeScope}
          onScopeChange={handleScopeChange}
        />
      )}
      {view === "home" && (
        <section className="surface home-recent">
          <h2>{t.previouslyConnectedDevices}</h2>
          {!devicesLoaded ? (
            <div className="empty-device">{t.loading}</div>
          ) : devices.length === 0 ? (
            <div className="empty-device">{t.noConnectedDevices}</div>
          ) : (
            <div className="home-recent-list">
              {devices.slice(0, 6).map((device) => {
                const presence = devicePresence[device.deviceCode];
                const deviceName = device.alias || presence?.name || t.unnamedDevice;
                const status = presence?.online === true
                  ? "online"
                  : presence?.online === false
                    ? "offline"
                    : "unknown";
                const statusLabel = status === "online"
                  ? t.online
                  : status === "offline"
                    ? t.offline
                    : t.unknown;

                return (
                  <button
                    className={`home-recent-device ${selectedCode === device.deviceCode ? "selected" : ""}`}
                    key={device.deviceId}
                    title={`${formatDeviceCode(device.deviceCode)} · ${deviceName} · ${statusLabel}`}
                    onClick={() => setSelectedCode(device.deviceCode)}
                    onDoubleClick={() => void connectSaved(device)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void connectSaved(device);
                      }
                    }}
                    onContextMenu={(event) => openDeviceMenu(event, device)}
                  >
                    <span className="home-recent-device-icon">
                      <OsLogo family={device.osFamily} />
                    </span>
                    <span className="home-recent-device-info">
                      <strong>{formatDeviceCode(device.deviceCode)}</strong>
                      <small>{deviceName}</small>
                    </span>
                    <span className={`home-recent-presence ${status}`} aria-label={statusLabel} />
                    <span className={`home-recent-connection ${device.connected ? "connected" : "disconnected"}`}>
                      {device.connected ? t.connected : t.disconnected}
                      {device.connected && device.connectionPath && (
                        <> · {device.connectionPath === "p2p" ? "P2P" : device.connectionPath === "relay" ? "Relay" : t.connectionPathUnknown}</>
                      )}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </section>
      )}
      {view === "remote" && (
        <>
          <RemoteConnectionPanel
            language={language}
            error={error}
            devices={devices}
            devicePresence={devicePresence}
            selectedCode={selectedCode}
            onSelect={(device) => setSelectedCode(device.deviceCode)}
            onConnect={(device) => void connectSaved(device)}
            onDeviceContextMenu={openDeviceMenu}
          />
          <DeviceDetailPanel
            language={language}
            selected={selected}
            presence={selected ? devicePresence[selected.deviceCode] : undefined}
            aliasDraft={aliasDraft}
            onAliasDraftChange={setAliasDraft}
            onRenameDevice={() => void renameDevice()}
            history={selected && (
              <Suspense fallback={<div className="empty-device">{t.loadingHistory}</div>}>
                <DeviceHistoryPanel key={selected.deviceCode} code={selected.deviceCode} language={language} />
              </Suspense>
            )}
            command={selected && (
              <Suspense fallback={<div className="empty-device">{t.loading}</div>}>
                <RemoteOperationsPanel
                  language={language}
                  selected={selected}
                  operation={operation}
                  onOperationChange={setOperation}
                  program={program}
                  onProgramChange={setProgram}
                  argumentsText={argumentsText}
                  onArgumentsTextChange={setArgumentsText}
                  cwd={cwd}
                  onCwdChange={setCwd}
                  submitting={submitting}
                  onRunCommand={() => void runCommand()}
                  transferDirection={transferDirection}
                  onTransferDirectionChange={setTransferDirection}
                  transferSource={transferSource}
                  onTransferSourceChange={setTransferSource}
                  transferDestination={transferDestination}
                  onTransferDestinationChange={setTransferDestination}
                  transferOverwrite={transferOverwrite}
                  onTransferOverwriteChange={setTransferOverwrite}
                  transfer={transfer}
                  startingTransfer={startingTransfer}
                  onStartTransfer={() => void startTransfer()}
                  onCancelTransfer={() => void cancelTransfer()}
                  onAuditChange={() => void refreshOperations()}
                />
              </Suspense>
            )}
          />
        </>
      )}

      {view === "activity" && (
        <Suspense fallback={<section className="surface activity-panel">{t.loadingHistory}</section>}>
          <ActivityPanel
            language={language}
            tasks={tasks}
            operations={operations}
            selectedId={selectedActivityId}
            onSelect={setSelectedActivityId}
            hasMore={hasMoreTasks || hasMoreOperations}
            loadingMore={loadingHistory}
            refreshing={refreshingHistory}
            refreshError={historyError}
            onLoadMore={() => void loadMoreHistory()}
            onRefresh={() => void refreshActivityManually()}
            onRefreshTask={(id) => void refreshTask(id)}
          />
        </Suspense>
      )}

      {contextMenu && menuDevice && createPortal(
        <div className="device-context-menu" role="menu" style={{ left: contextMenu.x, top: contextMenu.y }}>
          <button role="menuitem" autoFocus onClick={() => void connectSaved(menuDevice)}>
            <ArrowUpRight size={16} />{t.connect}
          </button>
          <button role="menuitem" disabled={!menuDevice.connected} onClick={() => {
            setContextMenu(null);
            setDisconnectError("");
            setDisconnectTarget(menuDevice);
          }}>
            <Unplug size={16} />{t.disconnectDevice}
          </button>
          <button role="menuitem" onClick={() => {
            setContextMenu(null);
            setRenameTarget(menuDevice);
            setRenameDraft(menuDevice.alias);
            setRenameError("");
          }}>
            <Pencil size={16} />{t.renameDevice}
          </button>
          <button role="menuitem" className="danger" onClick={() => {
            setContextMenu(null);
            setDeleteError("");
            setDeleteTarget(menuDevice);
          }}>
            <Trash2 size={16} />{t.deleteDevice}
          </button>
        </div>,
        document.body,
      )}

      {renameTarget && createPortal(
        <div className="device-dialog-backdrop" onMouseDown={() => { if (!renaming) setRenameTarget(null); }}>
          <form
            className="device-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="rename-device-title"
            onMouseDown={(event) => event.stopPropagation()}
            onSubmit={(event) => {
              event.preventDefault();
              void renameSavedDevice();
            }}
          >
            <h2 id="rename-device-title">{t.renameDevice}</h2>
            <strong>{formatDeviceCode(renameTarget.deviceCode)}</strong>
            <label className="rename-device-label" htmlFor="rename-device-input">{t.deviceAlias}</label>
            <input
              id="rename-device-input"
              autoFocus
              maxLength={64}
              value={renameDraft}
              onChange={(event) => setRenameDraft(event.target.value)}
            />
            {renameError && <p className="device-dialog-error" role="alert">{renameError}</p>}
            <div className="device-dialog-actions">
              <button type="button" disabled={renaming} onClick={() => setRenameTarget(null)}>{t.cancel}</button>
              <button type="submit" className="saved-connect-action" disabled={renaming}>
                {renaming ? t.loading : t.saveName}
              </button>
            </div>
          </form>
        </div>,
        document.body,
      )}

      {deleteTarget && createPortal(
        <div className="device-dialog-backdrop" onMouseDown={() => { if (!deleting) setDeleteTarget(null); }}>
          <div className="device-dialog" role="dialog" aria-modal="true" aria-labelledby="delete-device-title" onMouseDown={(event) => event.stopPropagation()}>
            <h2 id="delete-device-title">{t.deleteDeviceConfirm}</h2>
            <strong>{deleteTarget.alias || formatDeviceCode(deleteTarget.deviceCode)}</strong>
            <p>{t.deleteDeviceHint}</p>
            {deleteError && <p className="device-dialog-error" role="alert">{deleteError}</p>}
            <div className="device-dialog-actions">
              <button autoFocus disabled={deleting} onClick={() => setDeleteTarget(null)}>{t.cancel}</button>
              <button className="danger" disabled={deleting} onClick={() => void forgetDevice()}>{deleting ? t.loading : t.deleteDevice}</button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      {disconnectTarget && createPortal(
        <div className="device-dialog-backdrop" onMouseDown={() => { if (!disconnecting) setDisconnectTarget(null); }}>
          <div className="device-dialog" role="dialog" aria-modal="true" aria-labelledby="disconnect-device-title" onMouseDown={(event) => event.stopPropagation()}>
            <h2 id="disconnect-device-title">{t.disconnectDeviceConfirm}</h2>
            <strong>{disconnectTarget.alias || formatDeviceCode(disconnectTarget.deviceCode)}</strong>
            <p>{t.disconnectDeviceHint}</p>
            {disconnectError && <p className="device-dialog-error" role="alert">{disconnectError}</p>}
            <div className="device-dialog-actions">
              <button autoFocus disabled={disconnecting} onClick={() => setDisconnectTarget(null)}>{t.cancel}</button>
              <button className="danger" disabled={disconnecting} onClick={() => void disconnectDevice()}>
                {disconnecting ? t.loading : t.disconnectDevice}
              </button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      {savedConnection && (
        <SavedConnectionDialog
          language={language}
          connection={savedConnection}
          onClose={() => setSavedConnection(null)}
          onRetry={() => {
            const device = devices.find((item) => item.deviceId === savedConnection.deviceId);
            if (device) void connectSaved(device);
          }}
          onOpen={() => {
            setSavedConnection(null);
            onOpenRemote();
          }}
        />
      )}
    </>
  );
}
