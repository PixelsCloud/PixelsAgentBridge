import { invoke } from "@tauri-apps/api/core";
import type { ExecutionIdentity } from "./operatorTypes";

export type ExecutionSelection = { mode: "service" } | { mode: "user" | "desktop_user"; context_ref: string };
export type ExecutionEntry = {
  mode: ExecutionSelection["mode"];
  account_name: string;
  account_id: string | null;
  session_id: string | null;
  identity: ExecutionIdentity | null;
  selection: ExecutionSelection | null;
  unavailable_reason: string | null;
};
export type AppTarget = { kind: "id"; id: string } | { kind: "path"; path: string };
export type AppInfo = { name: string; app_id: string | null; path: string | null; source: string; instance: { process_id: number } | null };
export type AppSnapshot = { type: "list"; snapshot: { apps: AppInfo[]; truncated: boolean; warnings: string[]; execution_identity: ExecutionIdentity } }
  | { type: "action"; result: { request_accepted: boolean; execution_identity: ExecutionIdentity; notes: string[]; window_ready: boolean | null } };
export type ExecutionReply = {
  request_id: string;
  state: string;
  error: string | null;
  truncated: boolean;
  execution_context?: { identity?: ExecutionIdentity | null };
  data: { type: "execution_contexts"; entries: ExecutionEntry[] } | { type: "applications"; snapshot: AppSnapshot } | null;
};
export const isPending = (reply: ExecutionReply) => ["running", "queued", "unconfirmed"].includes(reply.state);
export function executionErrorMessage(cause: unknown): string {
  return typeof cause === "object" && cause !== null && "message" in cause ? String(cause.message) : String(cause);
}

export async function observeExecutionQuery(code: string, requestId: string): Promise<ExecutionReply> {
  return invoke("operator_execution_query_result", { code, requestId });
}

export async function executionQuery(code: string, query: unknown, requestId: string, observed?: (reply: ExecutionReply) => void): Promise<ExecutionReply> {
  let reply = await invoke<ExecutionReply>("operator_execution_query", { code, requestId, query });
  observed?.(reply);
  const deadline = Date.now() + 30_000;
  while (["running", "queued"].includes(reply.state) && Date.now() < deadline) {
    await new Promise((resolve) => window.setTimeout(resolve, 500));
    reply = await observeExecutionQuery(code, requestId);
    observed?.(reply);
  }
  return reply;
}
