import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from './api';

export type AccountIntent =
  | { operation_id: string; kind: 'password_reset'; revoke_sessions: boolean }
  | { operation_id: string; kind: 'session_revoke'; scope: 'all' | 'others' | 'session'; session_id?: string };
export interface AccountOperation {
  id: string; target_id: string; kind: string; sessions_revoked: number;
  completed_at: number; status: 'completed'; propagation: 'pending' | 'not_required';
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// Only these explicit public fields may survive a reload. No form values,
// passwords, token values or password fingerprints enter local storage.
export function readAccountIntent(value: string | null): AccountIntent | undefined {
  try {
    const parsed = JSON.parse(value ?? 'null');
    if (!parsed || typeof parsed.operation_id !== 'string' || !uuid.test(parsed.operation_id)) return;
    if (parsed.kind === 'password_reset' && typeof parsed.revoke_sessions === 'boolean') {
      return { operation_id: parsed.operation_id, kind: parsed.kind, revoke_sessions: parsed.revoke_sessions };
    }
    if (parsed.kind === 'session_revoke' && ['all', 'others', 'session'].includes(parsed.scope)
      && (parsed.scope === 'session' ? typeof parsed.session_id === 'string' && uuid.test(parsed.session_id) : parsed.session_id === undefined)) {
      return { operation_id: parsed.operation_id, kind: parsed.kind, scope: parsed.scope, ...(parsed.session_id ? { session_id: parsed.session_id } : {}) };
    }
  } catch { /* Invalid or unavailable saved state does not authorize any action. */ }
}

export function useAccountOperation(storageKey: string) {
  const [state, setState] = useState<{ intent?: AccountIntent; result?: AccountOperation; error?: string }>(() => {
    try { return { intent: readAccountIntent(localStorage.getItem(storageKey)) }; } catch { return {}; }
  });
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision(value => value + 1), []);
  const pending = !!state.intent && !state.result;
  const id = state.intent?.operation_id;
  useEffect(() => {
    try { state.intent ? localStorage.setItem(storageKey, JSON.stringify(state.intent)) : localStorage.removeItem(storageKey); } catch { /* The operation ID remains visible and copyable. */ }
  }, [state.intent, storageKey]);
  useEffect(() => {
    if (!id || !pending) return;
    const controller = new AbortController(); let inFlight = false;
    const query = async () => {
      if (inFlight) return; inFlight = true;
      try {
        const result = await api<AccountOperation>(`/account-operations/${id}`, { signal: controller.signal });
        if (!controller.signal.aborted && result.id === id) setState(current => current.intent?.operation_id === id ? { intent: current.intent, result } : current);
      } catch (error) {
        if (!controller.signal.aborted) setState(current => current.intent?.operation_id === id && !current.result ? { ...current, error: error instanceof ApiError ? error.code : 'networkError' } : current);
      } finally { inFlight = false; }
    };
    const poll = () => { if (document.visibilityState !== 'hidden') void query(); };
    void query(); const timer = window.setInterval(poll, 5000);
    document.addEventListener('visibilitychange', poll);
    return () => { controller.abort(); window.clearInterval(timer); document.removeEventListener('visibilitychange', poll); };
  }, [id, pending, revision]);
  return {
    ...state, pending, refresh,
    begin: (intent: AccountIntent) => setState({ intent }),
    accept: (result: AccountOperation) => setState(current => current.intent?.operation_id === result.id ? { intent: current.intent, result } : current),
    reject: (error: unknown) => {
      if (error instanceof ApiError && error.status < 500) setState({});
      else setState(current => ({ ...current, error: error instanceof ApiError ? error.code : 'networkError' }));
    },
  };
}
