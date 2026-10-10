import { createRoot } from 'react-dom/client';
import { ConfigProvider } from 'antd';
import { UpdateNotice } from '../src/UpdateNotice';
import '../src/App.css';

const callbacks = new Map<number, (event: unknown) => void>();
let nextCallback = 0;
const fixture = (window as any).fixture = {
  listeners: () => callbacks.size,
  emit: (payload: unknown) => {
    for (const callback of callbacks.values()) callback({ event: 'update-status-changed', payload });
  },
};
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: (callback: (event: unknown) => void) => {
    const id = ++nextCallback;
    callbacks.set(id, callback);
    return id;
  },
  invoke: async (command: string) => {
    if (command === 'plugin:event|listen') return 1;
    if (command === 'update_status') return null;
    return null;
  },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };

createRoot(document.getElementById('root')!).render(
  <ConfigProvider><main className="workspace" style={{ width: 820, height: 700 }}>
    <UpdateNotice language="zh-CN" onOpen={() => { document.body.dataset.open = 'true'; }} />
  </main></ConfigProvider>,
);
void fixture;
