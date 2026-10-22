import { createContext, useContext, useLayoutEffect, useMemo, useRef } from "react";

type Refresh = () => Promise<unknown>;

export function useAppRefreshRegistry() {
  return useMemo(() => {
    const handlers = new Set<Refresh>();
    return {
      register(handler: Refresh) {
        handlers.add(handler);
        return () => { handlers.delete(handler); };
      },
      async refresh() {
        const results = await Promise.allSettled([...handlers].map(handler => Promise.resolve().then(handler)));
        if (results.some(result => result.status === "rejected")) throw new Error("App refresh incomplete");
      },
    };
  }, []);
}

export const AppRefreshContext = createContext<ReturnType<typeof useAppRefreshRegistry> | null>(null);

// Refresh mounted data consumers without remounting forms or interrupting sessions.
export function useAppRefresh(refresh: Refresh) {
  const registry = useContext(AppRefreshContext);
  const latest = useRef(refresh);
  useLayoutEffect(() => { latest.current = refresh; });
  useLayoutEffect(() => registry?.register(() => latest.current()), [registry]);
}
