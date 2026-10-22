export const auditActions = ['admin.bootstrap','account.updated','account.relay_limit_updated','account.password_changed','account.password_reset','account.sessions_revoked','device.rename','device.associated','device.disassociated','relay_maintenance','relay_activate','relay_disable','relay.state_change','relay_config_reloaded','relay.capacity_updated','usage.exported','audit.exported','service.registration_updated'];
export const auditFilterKeys = ['q','from','until','actor','action','result','resource_kind','resource','page_size'] as const;
export interface AuditEvent {
  id: string; actor: string | null; actor_id: string | null; action: string; result: string; created_at: number;
  resource_kind: string | null; resource_key: string | null; resource_id: string | null; resource_name: string | null;
  request_id: string | null; operation_id: string | null;
  change_state: 'recorded' | 'not_applicable' | 'historical_unknown';
  before: Record<string, unknown> | null; after: Record<string, unknown> | null; details: Record<string, unknown> | null;
}
export interface AuditPageResult { items: AuditEvent[]; total: number; page_size: number; next_cursor: string | null; previous_cursor: string | null; snapshot_cursor: string; snapshot_frozen: boolean }
export function auditQuery(params: URLSearchParams) {
  const query = new URLSearchParams();
  for (const key of [...auditFilterKeys, 'cursor']) { const value = params.get(key); if (value) query.set(key, value); }
  return query;
}
export function auditFilters(params: URLSearchParams, values: Record<string, string | undefined>) {
  const next = new URLSearchParams();
  for (const key of auditFilterKeys) { const value = values[key] === undefined ? params.get(key) : values[key]; if (value) next.set(key, value); }
  return next;
}
