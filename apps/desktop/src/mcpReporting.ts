import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type DeviceRef = { tenant_id: string; device_id: string };
export type ToolCall = {
  id: string; tool: string; startedAtUnixMs: number; finishedAtUnixMs: number | null;
  deviceCode: string | null; deviceRef: DeviceRef | null; taskId: string | null; succeeded: boolean | null;
  requestId: string | null; terminalSessionId: string | null;
};
export type ReportedDevice = {
  deviceRef: DeviceRef; deviceCode: string | null; name: string | null; alias: string | null;
  phase: string; connectionPath: string | null; retryInMs: number | null;
  lastError: string | null; changedAtUnixMs: number;
  environment: { os_name: string; os_version: string; architecture: string } | null;
};
export type ReportedTask = {
  requestId: string; taskId: string | null; deviceRef: DeviceRef; capability: string; state: string;
  stage: string | null; createdAtUnixMs: number; startedAtUnixMs: number | null;
  finishedAtUnixMs: number | null; exitCode: number | null; errorCode: string | null;
};
export type ReportedOperation = {
  id: string; deviceRef: DeviceRef; deviceCode: string | null; kind: string; direction: string;
  source: string; destination: string; state: string; completedBytes: number; totalBytes: number;
  startedAtUnixMs: number; finishedAtUnixMs: number | null;
};
export type RuntimeReport = {
  account?: { localRevision: number; serverRevision: number | null; remoteRevision: number | null;
    policyVersion: number | null; user: { user_id: string; username: string } | null; error: string | null;
    relayNodes: { node_id: string; applied_policy_version: number | null; online: boolean }[] } | null;
  sessionId: string; identity: string; controlUrl: string; relayUrls: string[];
  controlPhase: string; lastError: string | null; sampledAtUnixMs: number;
  devices: ReportedDevice[]; tasks: ReportedTask[]; operations: ReportedOperation[];
};
export type ConnectedMcp = {
  report: {
    sessionId: string; processId: number; version: string; os: string; architecture: string;
    startedAtUnixMs: number; updatedAtUnixMs: number;
    clientName: string | null; clientVersion: string | null;
    activeCalls: ToolCall[]; recentCalls: ToolCall[]; runtime: RuntimeReport | null;
  };
  peerAddress: string; connectedAtUnixMs: number; lastSeenAtUnixMs: number;
};
export type ReportingStatus = {
  revision: number; running: boolean; listenAddress: string; error: string | null;
  count: number; clients: ConnectedMcp[];
};

export function useMcpReporting() {
  const [status, setStatus] = useState<ReportingStatus | null>(null);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;
    const accept = (next: ReportingStatus) => {
      if (active) {
        setStatus(current => !current || next.revision >= current.revision ? next : current);
        setError("");
      }
    };
    void (async () => {
      const stop = await listen<ReportingStatus>("mcp-reporting-changed", event => accept(event.payload));
      if (!active) { stop(); return; }
      unlisten = stop;
      accept(await invoke<ReportingStatus>("mcp_reporting_status"));
    })().catch(cause => { if (active) setError(String(cause)); });
    return () => { active = false; unlisten?.(); };
  }, []);
  return { status, error };
}
