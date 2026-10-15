import { createRoot } from 'react-dom/client';
import { ConfigProvider } from 'antd';
import { UpdateNotice } from '../src/UpdateNotice';
import { UpdatePanel } from '../src/UpdatePanel';
import '../src/App.css';

const callbacks = new Map<number, (event: unknown) => void>();
const events = new Map<number, string>();
const params = new URLSearchParams(location.search);
const showPanel = params.get('panel') === 'update';
const release = { version: '2.0.0', size: 1024, notes_zh: '测试更新', notes_en: 'Test update' };
let nextCallback = 0;
const fixture = (window as any).fixture = {
  calls: [] as string[],
  finishDownload: null as null | (() => void),
  listeners: () => callbacks.size,
  emit: (payload: unknown) => {
    for (const [id, callback] of callbacks) if (events.get(id) === 'update-status-changed') callback({ event: 'update-status-changed', payload });
  },
  progress: () => {
    for (const [id, callback] of callbacks) if (events.get(id) === 'update-download-progress') callback({ payload: { received: 1024, total: 1024 } });
  },
};
(window as any).__TAURI_INTERNALS__ = {
  transformCallback: (callback: (event: unknown) => void) => {
    const id = ++nextCallback;
    callbacks.set(id, callback);
    return id;
  },
  invoke: async (command: string, args: any) => {
    fixture.calls.push(command);
    if (command === 'plugin:event|listen') { events.set(args.handler, args.event); return args.handler; }
    if (command === 'update_status' || command === 'check_for_update') return showPanel ? release : null;
    if (command === 'download_update') {
      if (params.has('delayDownload')) await new Promise<void>(resolve => { fixture.finishDownload = resolve; });
      if (params.has('downloadError')) throw 'Downloaded installer checksum does not match';
    }
    if (command === 'install_update' && params.has('installError') && fixture.calls.filter(c => c === command).length === 1) {
      throw 'Could not start the installer';
    }
    return null;
  },
};
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };

createRoot(document.getElementById('root')!).render(
  <ConfigProvider><main className="workspace" style={{ width: 820, height: 700 }}>
    {showPanel ? <UpdatePanel language="zh-CN" />
      : <UpdateNotice language="zh-CN" onOpen={() => { document.body.dataset.open = 'true'; }} />}
  </main></ConfigProvider>,
);
void fixture;
