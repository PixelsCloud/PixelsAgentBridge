import { createRoot } from 'react-dom/client';
import { App, ConfigProvider } from 'antd';
import { BrowserHandoffNotice } from '../src/BrowserHandoffNotice';
import '../src/App.css';

let handoff = { reason: 'Sign in to continue', expiresAtUnixMs: Date.now() + 60 * 60 * 1000, ready: false, cancelled: false };
let cancelCalls = 0;
(window as any).fixture = { cancelCalls: () => cancelCalls };
(window as any).__TAURI_INTERNALS__ = {
  invoke: async (command: string) => {
    if (command === 'browser_handoff_status') return handoff;
    if (command === 'browser_handoff_cancel') {
      cancelCalls++;
      handoff = { ...handoff, cancelled: true };
      return handoff;
    }
    if (command === 'browser_handoff_complete') {
      handoff = { ...handoff, ready: true };
      return handoff;
    }
    return null;
  },
};

createRoot(document.getElementById('root')!).render(
  <ConfigProvider><App><BrowserHandoffNotice language="zh-CN" /></App></ConfigProvider>,
);
