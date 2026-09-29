export type ConnectedDevice = {
  deviceId: string;
  deviceCode: string;
  alias: string;
  osFamily: string;
  osReminder: string;
  connected: boolean;
  connectionPath?: "p2p" | "relay" | "unknown" | null;
  activeOperators?: number;
};

export type ScopeStatus = {
  userId: string;
  username: string;
  tenantId: string;
  teamName: string | null;
};

export type HistoryPage = {
  tasks: TaskEntry[];
  operations: OperationEntry[];
  taskBefore: string | null;
  operationBeforeStartedAtUnixMs: number | null;
  operationBeforeId: string | null;
  hasMoreTasks: boolean;
  hasMoreOperations: boolean;
};

export type OperatorBootstrap = HistoryPage & { devices: ConnectedDevice[] };

export type OperationEntry = {
  id: string;
  deviceCode: string;
  initiatedBy: string;
  kind: string;
  direction: "upload" | "download" | string;
  source: string;
  destination: string;
  overwrite: boolean;
  state: string;
  offset: number;
  size: number;
  startedAtUnixMs: number;
  finishedAtUnixMs: number | null;
  message: string | null;
  executionObservation: "active" | "unconfirmed" | "unknown" | null;
};

export type TaskUpdate = {
  state: string;
  complete: boolean;
  stdout: string;
  stderr: string;
  stdoutOffset: number;
  stderrOffset: number;
};

export type TaskEntry = TaskUpdate & {
  id: string;
  deviceCode: string;
  initiatedBy: string;
  program: string;
  args: string[];
  cwd: string | null;
  startedAtUnixMs: number;
};

export type TransferUpdate = {
  id: string;
  state: "running" | "completed" | "failed" | "cancel_requested";
  offset: number;
  size: number;
  message: string | null;
};
