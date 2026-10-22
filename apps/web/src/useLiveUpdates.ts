import { useEffect, useState } from 'react';
import { api, ApiError } from './api';

export function useLiveUpdates(enabled: boolean) {
  const [revision, setRevision] = useState(0);
  const [connected, setConnected] = useState(false);
  useEffect(() => {
    if (!enabled) { setConnected(false); return; }
    let stopped = false; let socket: WebSocket; let retry: ReturnType<typeof setTimeout>;
    let refresh: ReturnType<typeof setTimeout> | undefined;
    let last = 0; let attempts = 0; let lastMessage = Date.now(); let opened = false; let dirty = false;
    const flush = () => {
      clearTimeout(refresh); refresh = undefined;
      if (stopped || !dirty || document.visibilityState === 'hidden') return;
      dirty = false;
      setRevision(v => v + 1);
    };
    const schedule = () => {
      dirty = true;
      if (document.visibilityState === 'hidden') return;
      clearTimeout(refresh);
      refresh = setTimeout(flush, 150);
    };
    const connect = () => {
      if (stopped || !navigator.onLine) return;
      last = 0;
      socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/web/events`);
      const current = socket;
      socket.onopen = () => {
        if (stopped || socket !== current) return;
        attempts = 0; lastMessage = Date.now(); setConnected(true);
        if (opened) { dirty = true; flush(); }
        opened = true;
      };
      socket.onmessage = event => {
        if (stopped || socket !== current) return;
        try {
          const value = JSON.parse(event.data);
          if (value.type !== 'refresh' || !Number.isSafeInteger(value.sequence) || value.sequence <= last) return;
          last = value.sequence; lastMessage = Date.now();
          schedule();
        } catch { current.close(); }
      };
      socket.onclose = () => {
        if (stopped || socket !== current) return;
        setConnected(false); clearTimeout(refresh); refresh = undefined;
        // Check revocation before retrying, to avoid a silent expired-session loop.
        api('/session').catch(e => { if (e instanceof ApiError && e.status === 401) window.dispatchEvent(new Event('pab-session-expired')); });
        if (navigator.onLine) retry = setTimeout(connect, Math.min(1000 * 2 ** attempts++, 15000));
      };
      socket.onerror = () => current.close();
    };
    const offline = () => { clearTimeout(retry); setConnected(false); socket?.close(); };
    const online = () => { clearTimeout(retry); if (!socket || socket.readyState >= WebSocket.CLOSING) connect(); };
    const visibility = () => {
      if (document.visibilityState === 'hidden') { clearTimeout(refresh); refresh = undefined; }
      else if (socket?.readyState === WebSocket.OPEN) { dirty = true; flush(); }
    };
    window.addEventListener('offline', offline); window.addEventListener('online', online);
    document.addEventListener('visibilitychange', visibility);
    // A silent half-open socket must not leave the console showing live state forever.
    const watchdog = setInterval(() => { if (socket?.readyState === WebSocket.OPEN && Date.now() - lastMessage > 30000) socket.close(); }, 5000);
    connect();
    return () => { stopped = true; clearTimeout(retry); clearTimeout(refresh); clearInterval(watchdog); window.removeEventListener('offline', offline); window.removeEventListener('online', online); document.removeEventListener('visibilitychange', visibility); socket?.close(); };
  }, [enabled]);
  return { revision, connected };
}
