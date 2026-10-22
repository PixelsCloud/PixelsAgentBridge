export interface RelayIntent {
  node_id: string;
  state: 'active' | 'maintenance' | 'disabled';
  operation_id: string;
  allow_outage: boolean;
}

export interface RelayOperation {
  id: string;
  node_id: string;
  phase: 'waiting' | 'ready' | 'completed' | 'failed';
  action: string;
  actor: string;
  started_at_unix_ms: number;
  deadline_unix_ms: number;
  completed_at_unix_ms: number | null;
  server_now_unix_ms: number;
  capacity_sampled_at_unix_ms: number;
  remaining_host_bandwidth?: boolean; remaining_capacity_mbps: number | null;
  error_code: string | null;
  allow_outage: boolean;
}

// The server decides readiness. A zero countdown must never advance the phase.
export function remainingSeconds(operation: RelayOperation, elapsedMs: number): number {
  return Math.max(0, Math.ceil((operation.deadline_unix_ms - operation.server_now_unix_ms - elapsedMs) / 1000));
}
