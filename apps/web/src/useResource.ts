import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from './api';

export function useResource<T>(path: string) {
  const [loaded, setLoaded] = useState<{ path: string; value: T }>();
  const [error, setError] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const current = useRef(path);
  current.current = path;
  const refresh = useCallback(() => setRevision(v => v + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    // Keep the current snapshot readable during background invalidation refreshes.
    setLoading(current.current !== loaded?.path); setError(undefined);
    api<T>(path, { signal: controller.signal }).then(result => {
      if (!controller.signal.aborted && current.current === path) setLoaded({ path, value: result });
    }).catch(reason => {
      if (!controller.signal.aborted) {
        setError(reason instanceof ApiError ? reason.code : 'networkError');
        if (reason instanceof ApiError && [401, 403, 404].includes(reason.status)) setLoaded(undefined);
      }
    }).finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [path, revision]);
  return { data: loaded?.path === path ? loaded.value : undefined, error, loading, refresh };
}
