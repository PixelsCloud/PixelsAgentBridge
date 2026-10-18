// For an isolated acceptance Desktop only. The caller verifies its process,
// executable and exclusive ownership of the loopback debugging listener.
const [action, code, port = '39100'] = process.argv.slice(2);
if (!['open', 'status', 'closed'].includes(action) || !/^\d{9}$/.test(code) ||
    !/^\d{1,5}$/.test(port) || +port < 1024 || +port > 65535) throw new Error('Invalid acceptance arguments');
const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(r => r.json());
const viewers = pages.filter(p => p.type === 'page' && p.url.includes('desktopViewer='));
if (action === 'closed') {
  console.log(JSON.stringify(viewers.length === 0));
  process.exit(0);
}
const page = action === 'open'
  ? pages.find(p => p.type === 'page' && !p.url.includes('desktopViewer=') && p.url.includes('tauri'))
  : viewers.length === 1 ? viewers[0] : null;
if (!page && action === 'status' && viewers.length === 0) {
  console.log('null');
  process.exit(0);
}
if (!page) throw new Error('Missing or ambiguous owned acceptance WebView');
const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error('CDP connect timed out')), 5000);
  socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
try {
  // Window creation precedes WebView/Tauri bridge readiness. An absent bridge
  // is an observation to poll, never a reason to replay the open command.
  const expression = action === 'open'
    ? `(async()=>{await window.__TAURI_INTERNALS__.invoke('desktop_viewer_window',{code:${JSON.stringify(code)}});return {accepted:true}})()`
    : "window.__TAURI_INTERNALS__?.invoke ? window.__TAURI_INTERNALS__.invoke('desktop_viewer_start') : null";
  const result = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('CDP result timed out; inspect original window')), 30000);
    socket.addEventListener('message', event => {
      const reply = JSON.parse(event.data);
      if (reply.id === 1) { clearTimeout(timer); resolve(reply); }
    });
    socket.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
  });
  if (result.error || result.result?.exceptionDetails) throw new Error(JSON.stringify(result.error || result.result.exceptionDetails));
  console.log(JSON.stringify(result.result?.result?.value ?? null));
} finally { socket.close(); }
