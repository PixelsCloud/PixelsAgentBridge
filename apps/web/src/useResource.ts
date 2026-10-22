import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from './api';

// The resource already loads on mount; only subsequent live revisions invalidate it.
export function useLiveRefresh(refresh: () => void, revision: number) {
  const previous = useRef(revision);
  useEffect(() => {
    if (previous.current === revision) return;
    previous.current = revision;
    refresh();
  }, [refresh, revision]);
}

export function useResource<T>(path: string | null, { retainPreviousData = false }: { retainPreviousData?: boolean } = {}) {
  const [loaded, setLoaded] = useState<{ path: string; value: T }>();
  const snapshot = useRef<{ path: string; value: T; serialized: string | undefined } | undefined>(undefined);
  const [failure, setFailure] = useState<{ path: string; code: string }>();
  const [loading, setLoading] = useState(true);
  const [settledPath, setSettledPath] = useState<string>();
  const request = useRef<() => void>(() => {});
  const refresh = useCallback(() => request.current(), []);
  useEffect(() => {
    if (path === null) { request.current = () => {}; setLoading(false); setFailure(undefined); return; }
    let stopped = false; let pending = false; let queued = false; let controller: AbortController;
    const load = async () => {
      if (stopped) return;
      if (pending) { queued = true; return; }
      pending = true;
      const current = new AbortController(); controller = current;
      const timeout = setTimeout(() => current.abort(), 30_000);
      setLoading(snapshot.current?.path !== path);
      try {
        const value = await api<T>(path, { signal: current.signal });
        if (stopped) return;
        const serialized = JSON.stringify(value);
        if (snapshot.current?.path !== path || snapshot.current.serialized !== serialized) {
          snapshot.current = { path, value, serialized };
          setLoaded({ path, value });
        }
        setFailure(undefined);
      } catch (reason) {
        if (stopped) return;
        setFailure({ path, code: reason instanceof ApiError ? reason.code : 'networkError' });
        if (reason instanceof ApiError && [401, 403, 404].includes(reason.status)) {
          queued = false; snapshot.current = undefined; setLoaded(undefined);
        }
      } finally {
        clearTimeout(timeout); pending = false;
        if (!stopped) {
          setSettledPath(path); setLoading(false);
          if (queued) { queued = false; void load(); }
        }
      }
    };
    // Invalidation never cancels an active read. Notifications queue one trailing
    // read, so slow responses can complete and later changes are still fetched.
    setFailure(undefined); request.current = () => { void load(); }; void load();
    return () => { stopped = true; request.current = () => {}; controller?.abort(); };
  }, [path]);
  // Retention is opt-in for views that label the displayed snapshot's own bounds.
  // Report a changed query as pending during render, before its Effect starts.
  return {
    data: path !== null && (loaded?.path === path || retainPreviousData) ? loaded?.value : undefined,
    error: failure?.path === path ? failure.code : undefined,
    loading: path !== null && (loading || settledPath !== path), refresh,
  };
}
