import { lazy, Suspense, useEffect, useRef, useState, type SetStateAction } from "react";
import { App as AntdApp, Dropdown, Input, Modal, type MenuProps } from "antd";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ArrowUpRight, Copy, Monitor, Pencil, Trash2, Unplug, Terminal as TerminalIcon } from "lucide-react";
import { messages, type Language } from "./i18n";
import type { View } from "./App";
import type { ConnectedDevice, HistoryPage, OperatorBootstrap, OperationEntry, TaskEntry, TaskUpdate, TransferUpdate } from "./operatorTypes";
import { RemoteConnectionPanel } from "./RemoteConnectionPanel";
import { DeviceDetailPanel } from "./DeviceDetailPanel";
import type { OperationKind } from "./RemoteOperationsPanel";
import { executionErrorMessage, type ExecutionSelection } from "./executionQueries";
import { mergeTransferUpdate } from "./transferUpdates";
import { formatDeviceCode } from "./deviceCode";
import { HomeDeviceConnectionPanel } from "./HomeDeviceConnectionPanel";
import { OsLogo } from "./OsLogo";
import { SavedConnectionDialog, type SavedConnection } from "./SavedConnectionDialog";
import { deviceOnline } from "./deviceAvailability";

const ActivityPanel = lazy(() => import("./ActivityPanel").then(({ ActivityPanel }) => ({ default: ActivityPanel })));
const DeviceHistoryPanel = lazy(() => import("./DeviceHistoryPanel").then(({ DeviceHistoryPanel }) => ({ default: DeviceHistoryPanel })));
const RemoteOperationsPanel = lazy(() => import("./RemoteOperationsPanel").then(({ RemoteOperationsPanel }) => ({ default: RemoteOperationsPanel })));

export function OperatorPanel({ language, view, onOpenRemote, signedIn, onSignIn }: { language: Language; view: View; onOpenRemote: () => void; signedIn: boolean; onSignIn: () => void }) {
  const t = messages[language];
  const { notification } = AntdApp.useApp();
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [devices, setDevices] = useState<ConnectedDevice[]>([]);
  const [devicesLoaded, setDevicesLoaded] = useState(false);
  const [refreshingDevices, setRefreshingDevices] = useState(false);
  const [devicePresence, setDevicePresence] = useState<Record<string, { name: string; online: boolean | null }>>({});
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
  const [historyTotalCount, setHistoryTotalCount] = useState(0);
  const [taskBefore, setTaskBefore] = useState<string | null>(null);
  const [operationBeforeStartedAtUnixMs, setOperationBeforeStartedAtUnixMs] = useState<number | null>(null);
  const [operationBeforeId, setOperationBeforeId] = useState<string | null>(null);
  const [hasMoreTasks, setHasMoreTasks] = useState(false);
  const [hasMoreOperations, setHasMoreOperations] = useState(false);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [refreshingHistory, setRefreshingHistory] = useState(false);
  const [historyError, setHistoryError] = useState("");
  const [selectedActivityId, setSelectedActivityId] = useState("");
  const [historyDeviceCode, setHistoryDeviceCode] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [savedConnection, setSavedConnection] = useState<SavedConnection | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
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
  const connectionRevisionRef = useRef(0);
  const deviceRefreshRef = useRef(false);
  const presenceRequestRef = useRef(0);
  const historyRefreshRef = useRef<Promise<void> | null>(null);
  const refreshingRef = useRef(new Set<string>());
  const refreshAgainRef = useRef(new Set<string>());
  const selected = devices.find((device) => device.deviceCode === selectedCode);
  const connectedCodes = devices.filter((device) => device.connected).map((device) => device.deviceCode).join(",");
  const savedCodes = devices.map((device) => device.deviceCode).join(",");

  function updateDeviceConnections(update: SetStateAction<ConnectedDevice[]>) {
    connectionRevisionRef.current += 1;
    setDevices(update);
  }

  async function refreshDevicePresence(isClosed = () => false) {
    const request = ++presenceRequestRef.current;
    try {
      const statuses = await invoke<{ deviceCode: string; name: string; online: boolean | null }[]>(
        "operator_saved_device_presence",
      );
      if (isClosed() || request !== presenceRequestRef.current) return;
      setDevicePresence((current) => {
        const next = { ...current };
        for (const status of statuses) {
          next[status.deviceCode] = { name: status.name || current[status.deviceCode]?.name || "", online: status.online };
        }
        return next;
      });
    } catch (cause) {
      if (!isClosed() && request === presenceRequestRef.current) {
        setDevicePresence((current) => Object.fromEntries(Object.entries(current).map(([code, value]) =>
          [code, { ...value, online: null }])));
      }
      throw cause;
    }
  }

  async function refreshDeviceList() {
    if (deviceRefreshRef.current || !signedIn) return;
    deviceRefreshRef.current = true;
    setRefreshingDevices(true);
    setError("");
    const revision = connectionRevisionRef.current;
    try {
      const saved = await invoke<OperatorBootstrap>("operator_bootstrap");
      setDevices((current) => {
        const known = new Map(current.map((device) => [device.deviceCode, device]));
        const next = saved.devices.map((device) => {
          const previous = known.get(device.deviceCode);
          return previous ? { ...device, connected: previous.connected, connectionPath: previous.connectionPath } : device;
        });
        const savedCodes = new Set(next.map((device) => device.deviceCode));
        return revision === connectionRevisionRef.current ? next : [...next, ...current.filter((device) => !savedCodes.has(device.deviceCode))];
      });
      setDevicesLoaded(true);
      await refreshDevicePresence();
    } catch {
      setError(t.refreshDevicesFailed);
    } finally {
      deviceRefreshRef.current = false;
      setRefreshingDevices(false);
    }
  }

  useEffect(() => {
    if (view !== "remote") return;
    if (devices.some((device) => device.deviceCode === selectedCode)) return;
    setSelectedCode(devices.find((device) => device.connected)?.deviceCode ?? devices[0]?.deviceCode ?? "");
  }, [view, devices, selectedCode]);

  useEffect(() => {
    if (!signedIn || (view !== "home" && view !== "remote") || !savedCodes) return;
    let closed = false;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      try {
        await refreshDevicePresence(() => closed);
      } catch { /* Availability becomes unknown until a later refresh succeeds. */ }
      finally {
        refreshing = false;
      }
    };
    void refresh();
    const interval = window.setInterval(() => void refresh(), 15000);
    return () => {
      closed = true;
      window.clearInterval(interval);
    };
  }, [view, savedCodes, signedIn]);

  useEffect(() => {
    if (!signedIn || (view !== "home" && view !== "remote") || !savedCodes) return;
    let closed = false;
    let refreshing = false;
    const refresh = async () => {
      if (refreshing) return;
      refreshing = true;
      const revision = connectionRevisionRef.current;
      try {
        const statuses = await invoke<{ deviceCode: string; path: ConnectedDevice["connectionPath"] }[]>(
          "operator_connection_paths",
        );
        if (closed || revision !== connectionRevisionRef.current) return;
        const paths = new Map(statuses.map((status) => [status.deviceCode, status.path]));
        setDevices((current) => revision !== connectionRevisionRef.current ? current : current.map((device) => {
          if (!paths.has(device.deviceCode)) return device;
          const connectionPath = paths.get(device.deviceCode) ?? null;
          const connected = connectionPath !== null;
          return device.connected === connected && device.connectionPath === connectionPath
            ? device
            : { ...device, connected, connectionPath };
        }));
      } catch {
        if (!closed && revision === connectionRevisionRef.current) {
          setDevices((current) => revision !== connectionRevisionRef.current ? current : current.map((device) =>
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
  }, [view, savedCodes, signedIn]);

  useEffect(() => {
    if (!deleteTarget) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !deleting) setDeleteTarget(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [deleteTarget, deleting]);

  function deviceMenu(device: ConnectedDevice): MenuProps {
    return {
      items: [
        { key: "desktop-viewer", icon: <Monitor size={16} />, label: language === "en" ? "Remote desktop" : "远程桌面" },
        { key: "desktop-terminal", icon: <TerminalIcon size={16} />, label: language === "en" ? "Remote terminal" : "远程终端" },
        { key: "desktop-terminal-system", icon: <TerminalIcon size={16} />, label: language === "en" ? "Remote terminal (System)" : "远程终端（System）" },
        { key: "connect", icon: <ArrowUpRight size={16} />, label: t.connect },
        { key: "disconnect", icon: <Unplug size={16} />, label: t.disconnectDevice, disabled: !device.connected },
        { key: "rename", icon: <Pencil size={16} />, label: t.renameDevice },
        { key: "copy", icon: <Copy size={16} />, label: t.copyDeviceInfo },
        { type: "divider" },
        { key: "delete", icon: <Trash2 size={16} />, label: t.deleteDevice, danger: true },
      ],
      onClick: ({ key }) => {
        if (key === "desktop-viewer") void openRemoteDesktop(device);
        if (key === "desktop-terminal") void openDesktopTerminal(device, "console");
        if (key === "desktop-terminal-system") void openDesktopTerminal(device, "system");
        if (key === "connect") void connectSaved(device);
        if (key === "disconnect") {
          setDisconnectError("");
          setDisconnectTarget(device);
        }
        if (key === "rename") {
          setRenameTarget(device);
          setRenameDraft(device.alias);
          setRenameError("");
        }
        if (key === "copy") void copyDeviceInfo(device);
        if (key === "delete") {
          setDeleteError("");
          setDeleteTarget(device);
        }
      },
    };
  }

  async function openDesktopTerminal(device: ConnectedDevice, mode: "console" | "system") {
    if (!signedIn) { onSignIn(); return; }
    try {
      if (!device.connected) {
        const connected = await invoke<ConnectedDevice>("operator_connect_saved", { code: device.deviceCode });
        updateDeviceConnections((current) => [connected, ...current.filter((item) => item.deviceCode !== connected.deviceCode)]);
      }
      await invoke("desktop_terminal_window", { code: device.deviceCode, mode });
    } catch (cause) {
      if (/login required|account unavailable/i.test(String(cause))) { onSignIn(); return; }
      notification.error({ message: language === "en" ? "Could not open remote terminal" : "无法打开远程终端", description: String(cause), placement: "bottomRight" });
    }
  }

  async function openRemoteDesktop(device: ConnectedDevice) {
    if (!signedIn) { onSignIn(); return; }
    try {
      if (!device.connected) {
        const connected = await invoke<ConnectedDevice>("operator_connect_saved", { code: device.deviceCode });
        updateDeviceConnections((current) => [connected, ...current.filter((item) => item.deviceCode !== connected.deviceCode)]);
      }
      await invoke("desktop_viewer_window", { code: device.deviceCode });
    } catch (cause) {
      if (/login required|account unavailable/i.test(String(cause))) { onSignIn(); return; }
      notification.error({ message: language === "en" ? "Could not open remote desktop" : "无法打开远程桌面", description: String(cause), placement: "bottomRight" });
    }
  }

  async function copyDeviceInfo(device: ConnectedDevice) {
    const name = device.alias || devicePresence[device.deviceCode]?.name || t.unnamedDevice;
    const info = `${t.deviceCode}: ${device.deviceCode.replace(/\s/g, "")}\n${t.deviceInfoName}: ${name}`;
    try {
      await navigator.clipboard.writeText(info);
      notification.success({ message: t.deviceInfoCopied, placement: "bottomRight", duration: 2.5 });
    } catch {
      notification.error({ message: t.deviceInfoCopyFailed, placement: "bottomRight", duration: 2.5 });
    }
  }

  async function connectSaved(device: ConnectedDevice, replacementPassword?: string) {
    if (!signedIn) { onSignIn(); return; }
    const attempt = ++savedConnectionAttemptRef.current;
    setSavedConnection({
      mode: "saved",
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
      const connected = replacementPassword === undefined
        ? await invoke<ConnectedDevice>("operator_connect_saved", { code: device.deviceCode })
        : await invoke<ConnectedDevice>("operator_connect", { code: device.deviceCode, password: replacementPassword });
      updateDeviceConnections((current) => [connected, ...current.filter((item) => item.deviceCode !== connected.deviceCode)]);
      setSelectedCode(connected.deviceCode);
      setAliasDraft(connected.alias);
      setSavedConnection((current) => current?.attempt === attempt
        ? { ...current, phase: "connected", step: 3, message: "" }
        : current);
    } catch (cause) {
      if (/login required|account unavailable/i.test(String(cause))) {
        setSavedConnection(null);
        onSignIn();
        return;
      }
      if (savedConnectionAttemptRef.current === attempt) {
        updateDeviceConnections((current) => current.map((item) => item.deviceId === device.deviceId
          ? { ...item, connected: false, connectionPath: null }
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
      updateDeviceConnections((current) => current.map((device) => device.deviceId === disconnectTarget.deviceId
        ? { ...device, connected: false, connectionPath: null }
        : device));
      setDisconnectTarget(null);
    } catch (cause) {
      setDisconnectError(`${t.disconnectFailed}: ${String(cause)}`);
    } finally {
      setDisconnecting(false);
    }
  }


  async function refreshOperations() {
    const saved = await invoke<OperationEntry[]>("operator_operations");
    setTransfer((current) => {
      if (!current) return current;
      const record = saved.find((item) => item.id === current.id);
      if (!record || !["running", "cancel_requested", "completed", "failed", "cancelled", "interrupted", "unconfirmed"].includes(record.state)) {
        return current;
      }
      return mergeTransferUpdate(current, {
        ...current,
        state: record.executionObservation === "unconfirmed" ? "unconfirmed" : record.state as TransferUpdate["state"],
        offset: record.offset, size: record.size, message: record.message, executionIdentity: record.executionIdentity,
      });
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
      setHistoryTotalCount(page.totalCount);
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
    if (loadingHistory || (!hasMoreTasks && !hasMoreOperations)) return false;
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
      return true;
    } catch {
      setHistoryError(t.historyLoadFailed);
      return false;
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
            executionIdentity: update.executionIdentity,
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
        setHistoryTotalCount(saved.totalCount);
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
        updateDeviceConnections((current) => current.map((device) => device.deviceId === event.payload.deviceId ? {
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
          if (!current || current.id !== next.id) return current;
          return mergeTransferUpdate(current, next);
        });
        setOperations((current) => current.map((item) => item.id === event.payload.id ? {
          ...item,
          state: event.payload.state,
          offset: event.payload.offset,
          size: event.payload.size,
          executionIdentity: event.payload.executionIdentity,
          message: event.payload.message,
          executionObservation: ["cancel_requested", "unconfirmed"].includes(event.payload.state) ? "unconfirmed" : event.payload.state === "running" ? item.executionObservation : null,
          finishedAtUnixMs: ["running", "cancel_requested", "unconfirmed"].includes(event.payload.state) ? null : Date.now(),
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
    if (operation === "windows" && selected && !/windows|mac/i.test(selected.osFamily)) {
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

  async function connect(deviceCode = code, devicePassword = password) {
    if (!signedIn) { onSignIn(); return; }
    if (connecting || deviceCode.length !== 9 || !devicePassword) return;
    const attempt = ++savedConnectionAttemptRef.current;
    setPassword(devicePassword);
    setConnecting(true);
    setSavedConnection({
      mode: "manual",
      deviceId: "",
      deviceCode,
      name: "",
      phase: "connecting",
      step: 1,
      message: "",
      attempt,
    });
    try {
      const device = await invoke<ConnectedDevice>("operator_connect", {
        code: deviceCode,
        password: devicePassword,
      });
      updateDeviceConnections((current) => [device, ...current.filter((item) => item.deviceCode !== device.deviceCode)]);
      setSelectedCode(device.deviceCode);
      setCode("");
      setPassword("");
      setAliasDraft(device.alias);
      setSavedConnection((current) => current?.attempt === attempt
        ? { ...current, deviceId: device.deviceId, name: device.alias || devicePresence[device.deviceCode]?.name || "", phase: "connected", step: 3 }
        : current);
    } catch (cause) {
      setSavedConnection((current) => current?.attempt === attempt
        ? { ...current, phase: "failed", message: `${t.addDeviceFailed}: ${String(cause)}` }
        : current);
      if (/login required|account unavailable/i.test(String(cause))) {
        setSavedConnection(null);
        onSignIn();
      }
    } finally {
      setConnecting(false);
    }
  }

  async function runCommand(execution: ExecutionSelection) {
    if (!selected?.connected || !program.trim()) return;
    setSubmitting(true);
    setError("");
    try {
      const id = await invoke<string>("operator_run_command", {
        execution,
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
    } catch (cause) {
      setError(`${t.commandFailed}: ${String(cause)}`);
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

  async function startTransfer(execution: ExecutionSelection) {
    if (!selected?.connected || !transferSource.trim() || !transferDestination.trim() || startingTransfer) return;
    const id = crypto.randomUUID();
    const initial: TransferUpdate = { id, deviceCode: selected.deviceCode, state: "running", offset: 0, size: 0, message: null };
    setTransfer(initial); setStartingTransfer(true); setError("");
    try {
      const next = await invoke<TransferUpdate>("operator_start_transfer", {
        code: selected.deviceCode, requestId: id, execution, direction: transferDirection,
        source: transferSource.trim(), destination: transferDestination.trim(), overwrite: transferOverwrite,
      });
      setTransfer((current) => current?.id === id ? mergeTransferUpdate(current, next) : current);
      setSelectedActivityId(id);
      void refreshOperations();
    } catch (cause) {
      const knownRejection = typeof cause === "object" && cause !== null && "outcome" in cause && cause.outcome === "not_submitted";
      const message = executionErrorMessage(cause);
      setTransfer((current) => current?.id !== id ? current : knownRejection ? null : mergeTransferUpdate(current, { ...initial, state: "unconfirmed", message }));
      setError(`${t.transferFailed}: ${message}`);
    } finally { setStartingTransfer(false); }
  }

  async function inspectTransfer() {
    if (!transfer) return;
    const current = transfer;
    try {
      const next = await invoke<TransferUpdate>("operator_transfer_result", { id: current.id, code: current.deviceCode });
      setTransfer((value) => value?.id === current.id ? mergeTransferUpdate(value, next) : value);
      void refreshOperations();
    } catch (cause) { setError(executionErrorMessage(cause)); }
  }

  async function cancelTransfer() {
    if (!transfer || transfer.state !== "running") return;
    const current = transfer;
    try {
      const next = await invoke<TransferUpdate>("operator_cancel_transfer", { id: current.id, code: current.deviceCode });
      setTransfer((value) => value?.id === current.id ? mergeTransferUpdate(value, next) : value);
    } catch (cause) { setError(executionErrorMessage(cause)); }
  }

  return (
    <>
      {view === "home" && (
        <HomeDeviceConnectionPanel
          signedIn={signedIn}
          onSignIn={onSignIn}
          language={language}
          code={code}
          password={password}
          connecting={connecting}
          onCodeChange={setCode}
          onPasswordChange={setPassword}
          onConnect={() => void connect()}
        />
      )}
      {view === "home" && (
        <section className="surface home-recent">
          <h2>{t.addedDevices}</h2>
          {!devicesLoaded ? (
            <div className="empty-device">{t.loading}</div>
          ) : devices.length === 0 ? (
            <div className="empty-device">{t.noConnectedDevices}</div>
          ) : (
            <div className="home-recent-list">
              {devices.slice(0, 6).map((device) => {
                const presence = devicePresence[device.deviceCode];
                const deviceName = device.alias || presence?.name || t.unnamedDevice;
                const availability = deviceOnline(device, presence);
                const status = availability === true
                  ? "online"
                  : availability === false
                    ? "offline"
                    : "unknown";
                const statusLabel = status === "online"
                  ? t.online
                  : status === "offline"
                    ? t.offline
                    : t.unknown;

                return (
                  <Dropdown key={device.deviceId} trigger={["contextMenu"]} menu={deviceMenu(device)}>
                  <button
                    className={`home-recent-device ${selectedCode === device.deviceCode ? "selected" : ""}`}
                    title={`${formatDeviceCode(device.deviceCode)} · ${deviceName} · ${statusLabel}`}
                    onClick={() => setSelectedCode(device.deviceCode)}
                    onDoubleClick={() => void connectSaved(device)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void connectSaved(device);
                      }
                    }}
                  >
                    <span className="home-recent-device-icon">
                      <OsLogo family={device.osFamily} />
                    </span>
                    <span className="home-recent-device-info">
                      <strong>{formatDeviceCode(device.deviceCode)}</strong>
                      <small>{deviceName}</small>
                    </span>
                    <span className={`home-recent-presence ${status}`} aria-label={statusLabel} />
                    <span className={`home-recent-connection ${availability === true ? "connected" : "disconnected"}`}>
                      {device.connected ? t.connected : statusLabel}
                      {device.connected && device.connectionPath && (
                        <> · {device.connectionPath === "p2p" ? "P2P" : device.connectionPath === "relay" ? "Relay" : t.connectionPathUnknown}</>
                      )}
                    </span>
                  </button>
                  </Dropdown>
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
            refreshing={refreshingDevices || !signedIn}
            onRefresh={() => void refreshDeviceList()}
            devicePresence={devicePresence}
            selectedCode={selectedCode}
            onSelect={(device) => setSelectedCode(device.deviceCode)}
            onConnect={(device) => void connectSaved(device)}
            onDeviceMenu={deviceMenu}
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
                  key={selected.deviceCode}
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
                  onRunCommand={(execution) => void runCommand(execution)}
                  transferDirection={transferDirection}
                  onTransferDirectionChange={setTransferDirection}
                  transferSource={transferSource}
                  onTransferSourceChange={setTransferSource}
                  transferDestination={transferDestination}
                  onTransferDestinationChange={setTransferDestination}
                  transferOverwrite={transferOverwrite}
                  onTransferOverwriteChange={setTransferOverwrite}
                  transfer={transfer?.deviceCode === selected.deviceCode ? transfer : null}
                  startingTransfer={startingTransfer}
                  onStartTransfer={(execution) => void startTransfer(execution)}
                  onInspectTransfer={() => void inspectTransfer()}
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
          {historyDeviceCode ? <DeviceHistoryPanel
            key={historyDeviceCode}
            code={historyDeviceCode}
            language={language}
            embedded={false}
            deviceOptions={devices.map((device) => ({ code: device.deviceCode, label: `${device.alias || devicePresence[device.deviceCode]?.name || formatDeviceCode(device.deviceCode)} · ${formatDeviceCode(device.deviceCode)}` }))}
            onDeviceChange={setHistoryDeviceCode}
          /> : <ActivityPanel
            language={language}
            tasks={tasks}
            operations={operations}
            totalCount={historyTotalCount}
            selectedId={selectedActivityId}
            onSelect={setSelectedActivityId}
            hasMore={hasMoreTasks || hasMoreOperations}
            hasMoreTasks={hasMoreTasks}
            hasMoreOperations={hasMoreOperations}
            loadingMore={loadingHistory}
            refreshing={refreshingHistory}
            refreshError={historyError}
            onLoadMore={loadMoreHistory}
            deviceOptions={devices.map((device) => ({ code: device.deviceCode, label: `${device.alias || devicePresence[device.deviceCode]?.name || formatDeviceCode(device.deviceCode)} · ${formatDeviceCode(device.deviceCode)}` }))}
            deviceCode=""
            onDeviceChange={setHistoryDeviceCode}
            onRefresh={() => void refreshActivityManually()}
            onRefreshTask={(id) => void refreshTask(id)}
          />}
        </Suspense>
      )}

      <Modal open={Boolean(renameTarget)} title={t.renameDevice} width={400}
        onCancel={() => { if (!renaming) setRenameTarget(null); }}
        onOk={() => void renameSavedDevice()} okText={t.saveName} okButtonProps={{ disabled: !renameTarget || renameDraft.trim() === renameTarget.alias }}
        confirmLoading={renaming} cancelText={t.cancel} maskClosable={false} keyboard={false}>
        {renameTarget && <>
          <strong>{formatDeviceCode(renameTarget.deviceCode)}</strong>
          <label className="rename-device-label" htmlFor="rename-device-input">{t.deviceAlias}</label>
          <Input id="rename-device-input" maxLength={64} value={renameDraft}
            onChange={(event) => setRenameDraft(event.target.value)} onPressEnter={() => void renameSavedDevice()} />
          {renameError && <p className="device-dialog-error" role="alert">{renameError}</p>}
        </>}
      </Modal>

      <Modal open={Boolean(deleteTarget)} title={t.deleteDeviceConfirm} width={400}
        onCancel={() => { if (!deleting) setDeleteTarget(null); }} onOk={() => void forgetDevice()}
        okText={t.deleteDevice} okType="danger" confirmLoading={deleting} cancelText={t.cancel} maskClosable={false} keyboard={false}>
        {deleteTarget && <>
          <strong>{deleteTarget.alias || formatDeviceCode(deleteTarget.deviceCode)}</strong>
          <p>{t.deleteDeviceHint}</p>
          {deleteError && <p className="device-dialog-error" role="alert">{deleteError}</p>}
        </>}
      </Modal>

      <Modal open={Boolean(disconnectTarget)} title={t.disconnectDeviceConfirm} width={400}
        onCancel={() => { if (!disconnecting) setDisconnectTarget(null); }} onOk={() => void disconnectDevice()}
        okText={t.disconnectDevice} okType="danger" confirmLoading={disconnecting} cancelText={t.cancel} maskClosable={false} keyboard={false}>
        {disconnectTarget && <>
          <strong>{disconnectTarget.alias || formatDeviceCode(disconnectTarget.deviceCode)}</strong>
          <p>{t.disconnectDeviceHint}</p>
          {disconnectError && <p className="device-dialog-error" role="alert">{disconnectError}</p>}
        </>}
      </Modal>

      {savedConnection && (
        <SavedConnectionDialog
          language={language}
          connection={savedConnection}
          onClose={() => setSavedConnection(null)}
          onRetry={(replacementPassword) => {
            if (savedConnection.mode === "manual") {
              void connect(savedConnection.deviceCode, replacementPassword ?? password);
            } else {
              const device = devices.find((item) => item.deviceId === savedConnection.deviceId);
              if (device) void connectSaved(device, replacementPassword);
            }
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
