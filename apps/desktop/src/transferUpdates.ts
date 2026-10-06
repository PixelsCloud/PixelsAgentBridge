import type { TransferUpdate } from "./operatorTypes";

const terminal = (state: TransferUpdate["state"]) => ["completed", "failed", "cancelled", "interrupted"].includes(state);

// Tauri events can beat the invocation response. A late acceptance or progress
// response must not turn a finished transfer back into a running one.
export function mergeTransferUpdate(current: TransferUpdate, next: TransferUpdate): TransferUpdate {
  if (current.id !== next.id || current.deviceCode !== next.deviceCode) return current;
  if (terminal(current.state)) return current;
  return {
    ...next,
    state: current.state === "cancel_requested" && next.state === "running" ? current.state : next.state,
    offset: Math.max(current.offset, next.offset), size: Math.max(current.size, next.size),
    executionIdentity: next.executionIdentity ?? current.executionIdentity,
  };
}
