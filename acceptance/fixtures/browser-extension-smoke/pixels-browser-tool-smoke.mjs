// Test the pinned Pixels-owned Playwright runtime against a local, disposable site.
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, readdir, realpath, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import { fileURLToPath } from 'node:url';

const [node, cli, executable, channel = 'chrome'] = process.argv.slice(2);
if (!node || !cli || !executable || !['chrome', 'msedge'].includes(channel)) {
  throw new Error('node, cli, executable and Chrome/Edge channel required');
}
const root = await mkdtemp(path.join(tmpdir(), 'pixels-browser-tools-'));
const output = path.join(root, 'output');
process.argv[2] = '17841';
await import('./acceptance-server.mjs');
const child = spawn(node, [cli, '--browser', channel, '--headless', '--user-data-dir', path.join(root, 'profile'),
  '--executable-path', executable, '--no-webmcp', '--image-responses=omit', '--caps', 'vision,pdf',
  '--snapshot-boxes', '--output-dir', output], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true });
const pending = new Map();
let sequence = 0;
readline.createInterface({ input: child.stdout }).on('line', line => {
  let value;
  try { value = JSON.parse(line); } catch { return; }
  pending.get(value.id)?.(value);
  pending.delete(value.id);
});
child.stderr.on('data', bytes => process.stderr.write(bytes));
function call(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`${method} timed out`)); }, 30_000);
    pending.set(id, value => {
      clearTimeout(timer);
      value.error ? reject(new Error(value.error.message)) : resolve(value.result);
    });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}
async function tool(name, args = {}) {
  const reply = await call('tools/call', { name, arguments: args });
  const text = reply.content?.filter(item => item.type === 'text').map(item => item.text).join('\n') ?? '';
  if (reply.isError) throw new Error(`${name}: ${text.slice(0, 500)}`);
  return text;
}
function expect(text, fragment, step) {
  if (!text.includes(fragment)) throw new Error(`${step}: ${fragment} missing from ${text.slice(0, 300)}`);
}

let failure;
try {
  await call('initialize', { protocolVersion: '2025-03-26', capabilities: {},
    clientInfo: { name: 'Pixels browser tool smoke', version: '1' } });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  const available = (await call('tools/list')).tools.map(item => item.name);
  for (const name of ['browser_snapshot', 'browser_fill_form', 'browser_file_upload',
    'browser_take_screenshot', 'browser_pdf_save']) {
    if (!available.includes(name)) throw new Error(`upstream tool ${name} missing`);
  }
  await tool('browser_tabs', { action: 'list' });
  await tool('browser_navigate', { url: 'http://127.0.0.1:17841/' });
  expect(await tool('browser_snapshot'), 'Count: 0', 'navigate');
  await tool('browser_find', { text: 'Save' });
  await tool('browser_fill_form', { fields: [{ name: 'Name', type: 'textbox', target: '#name', value: 'Pixels' }] });
  await tool('browser_select_option', { target: '#color', values: ['blue'] });
  await tool('browser_evaluate', { target: '#agree', element: 'Agree checkbox',
    function: '(element) => { if (!element.checked) element.click(); return element.checked; }' });
  await tool('browser_click', { target: '#form button', element: 'Save' });
  expect(await tool('browser_snapshot'), 'Saved Pixels, true, blue', 'form');
  await tool('browser_hover', { target: '#hover', element: 'Hover here' });
  await tool('browser_find', { text: 'Menu is visible' });
  await tool('browser_drag', { startTarget: '#drag', endTarget: '#drop' });
  await tool('browser_wait_for', { text: 'fixture' });
  await tool('browser_press_key', { key: 'PageDown' });
  await tool('browser_evaluate', { target: '#items', element: 'Table',
    function: '(element) => [...element.querySelectorAll("tr")].map(row => row.innerText)' });
  const shot = path.join(output, 'smoke.jpeg');
  await tool('browser_take_screenshot', { type: 'jpeg', scale: 'css', filename: shot });
  if ((await stat(shot)).size < 1000) throw new Error('screenshot not saved');
  const downloadReport = await tool('browser_click', { target: '#download', element: 'Download result' });
  expect(downloadReport, '- Downloaded file ', 'download event');
  const downloads = await readdir(output);
  if (!downloads.includes('result.txt')) throw new Error(`download not saved: ${downloads}`);
  expect(await readFile(path.join(output, 'result.txt'), 'utf8'), 'Pixels browser acceptance fixture', 'download');
  await tool('browser_click', { target: '#dialog', element: 'Open alert' });
  await tool('browser_handle_dialog', { accept: true });
  await tool('browser_click', { target: '#file', element: 'Choose file' });
  await tool('browser_file_upload', { paths: [path.join(path.dirname(fileURLToPath(import.meta.url)), 'acceptance.html')] });
  expect(await tool('browser_snapshot'), 'acceptance.html', 'upload');
  await tool('browser_tabs', { action: 'new', url: 'http://127.0.0.1:17841/second' });
  expect(await tool('browser_snapshot'), 'Second page', 'new tab');
  await tool('browser_tabs', { action: 'select', index: 0 });
  await tool('browser_mouse_move_xy', { x: 80, y: 80 });
  await tool('browser_mouse_wheel', { deltaX: 0, deltaY: 30 });
  await tool('browser_drop', { target: '#drop', data: { 'text/plain': 'dropped fixture' } });
  expect(await tool('browser_snapshot'), 'dropped fixture', 'drop');
  await tool('browser_console_messages', { level: 'error' });
  await tool('browser_network_requests', { static: false });
  await tool('browser_pdf_save');
  console.log(`PIXELS_BROWSER_TOOLS_OK ${channel}`);
} catch (error) {
  failure = error;
} finally {
  try { await tool('browser_close'); } catch { /* Process may have exited. */ }
  child.kill();
  const resolved = await realpath(root);
  const temporary = await realpath(tmpdir());
  if (!resolved.startsWith(temporary + path.sep) || !path.basename(resolved).startsWith('pixels-browser-tools-')) {
    throw new Error('refusing to remove unexpected browser fixture directory');
  }
  await rm(resolved, { recursive: true, force: true });
}
if (failure) console.error(failure);
process.exit(failure ? 1 : 0);
