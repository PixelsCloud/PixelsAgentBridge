import { useEffect, useState } from 'react';
import { api, ApiError } from './api';

export function useLiveUpdates(enabled: boolean) {
  const [revision, setRevision] = useState(0);
  const [connected, setConnected] = useState(false);
  useEffect(() => {
    if (!enabled) { setConnected(false); return; }
    let stopped = false; let socket: WebSocket; let retry: ReturnType<typeof setTimeout>;
    let refresh: ReturnType<typeof setTimeout>; let last = 0; let attempts = 0; let lastMessage = Date.now();
    const connect = () => {
      if (stopped || !navigator.onLine) return;
      last = 0;
      socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/web/events`);
      const current = socket;
      socket.onopen = () => { if (!stopped) { attempts = 0; lastMessage = Date.now(); setConnected(true); setRevision(v => v + 1); } };
      socket.onmessage = event => {
        try {
          const value = JSON.parse(event.data);
          if (value.type !== 'refresh' || !Number.isSafeInteger(value.sequence) || value.sequence <= last) return;
          last = value.sequence; lastMessage = Date.now();
          clearTimeout(refresh); refresh = setTimeout(() => { if (!stopped) setRevision(v => v + 1); }, 150);
        } catch { current.close(); }
      };
      socket.onclose = () => {
        if (stopped) return; setConnected(false);
        // Check revocation before retrying, to avoid a silent expired-session loop.
        api('/session').catch(e => { if (e instanceof ApiError && e.status === 401) window.dispatchEvent(new Event('pab-session-expired')); });
        if (navigator.onLine) retry = setTimeout(connect, Math.min(1000 * 2 ** attempts++, 15000));
      };
      socket.onerror = () => current.close();
    };
    const offline = () => { clearTimeout(retry); setConnected(false); socket?.close(); };
    const online = () => { clearTimeout(retry); if (!socket || socket.readyState >= WebSocket.CLOSING) connect(); };
    window.addEventListener('offline', offline); window.addEventListener('online', online);
    // A silent half-open socket must not leave the console showing live state forever.
    const watchdog = setInterval(() => { if (socket?.readyState === WebSocket.OPEN && Date.now() - lastMessage > 30000) socket.close(); }, 5000);
    connect();
    return () => { stopped = true; clearTimeout(retry); clearTimeout(refresh); clearInterval(watchdog); window.removeEventListener('offline', offline); window.removeEventListener('online', online); socket?.close(); };
  }, [enabled]);
  return { revision, connected };
}
