// Prove that Pixels-owned Playwright MCP browser profiles retain a test-site login.
import { spawn } from 'node:child_process';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import readline from 'node:readline';

const [node, cli, executable, channel = 'chrome'] = process.argv.slice(2);
if (!node || !cli || !executable || !['chrome', 'msedge'].includes(channel)) {
  throw new Error('node, cli, executable and supported channel required');
}
const root = await mkdtemp(path.join(tmpdir(), 'pixels-browser-persist-'));
const profile = path.join(root, 'profile');
process.argv[2] = '17841';
await import('./acceptance-server.mjs');

async function session(action, profilePath = profile) {
  const child = spawn(node, [cli, '--browser', channel, '--headless', '--user-data-dir', profilePath,
    '--executable-path', executable, '--no-webmcp', '--image-responses=omit'], {
    stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
  });
  let sequence = 0;
  const pending = new Map();
  const lines = readline.createInterface({ input: child.stdout });
  lines.on('line', line => {
    let value;
    try { value = JSON.parse(line); } catch { return; }
    pending.get(value.id)?.(value);
    pending.delete(value.id);
  });
  child.stderr.on('data', bytes => process.stderr.write(bytes));
  function call(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++sequence;
      pending.set(id, value => value.error ? reject(new Error(value.error.message)) : resolve(value.result));
      child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    });
  }
  async function tool(name, args = {}) {
    const reply = await call('tools/call', { name, arguments: args });
    const content = reply.content?.filter(item => item.type === 'text').map(item => item.text).join('\n') ?? '';
    if (reply.isError) throw new Error(`${name}: ${content}`);
    return content;
  }
  const deadline = setTimeout(() => child.kill(), 60_000);
  try {
    await call('initialize', { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'Pixels persistent browser probe', version: '1' } });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
    await tool('browser_tabs', { action: 'list' });
    await action(tool);
  } finally {
    try { await tool('browser_close'); } catch { /* The process may have exited. */ }
    clearTimeout(deadline);
    child.kill();
    lines.close();
  }
}

let failure;
try {
  await session(async tool => {
    await tool('browser_navigate', { url: 'http://127.0.0.1:17841/account' });
    const before = await tool('browser_snapshot');
    if (!before.includes('Sign in required')) throw new Error('fresh profile was already signed in');
    await tool('browser_type', { target: 'input[name="username"]', text: 'demo' });
    await tool('browser_type', { target: 'input[name="password"]', text: 'demo' });
    await tool('browser_click', { target: 'button', element: 'Sign in' });
    const after = await tool('browser_snapshot');
    if (!after.includes('Signed in as demo')) throw new Error('fixture login failed');
  });
  await session(async tool => {
    await tool('browser_navigate', { url: 'http://127.0.0.1:17841/account' });
    const afterRestart = await tool('browser_snapshot');
    if (!afterRestart.includes('Signed in as demo')) throw new Error('login did not survive browser restart');
  });
  await session(async tool => {
    await tool('browser_navigate', { url: 'http://127.0.0.1:17841/account' });
    const isolated = await tool('browser_snapshot');
    if (!isolated.includes('Sign in required')) throw new Error('another profile reused the login');
  }, path.join(root, 'other-profile'));
  await session(async tool => {
    await tool('browser_navigate', { url: 'http://127.0.0.1:17841/logout' });
    const signedOut = await tool('browser_snapshot');
    if (!signedOut.includes('Sign in required')) throw new Error('site logout did not clear the login');
  });
  console.log('PIXELS_PERSISTENT_PROFILE_OK');
} catch (error) {
  failure = error;
} finally {
  const resolvedRoot = await realpath(root);
  const resolvedTemp = await realpath(tmpdir());
  if (!resolvedRoot.startsWith(resolvedTemp + path.sep) || path.basename(resolvedRoot).startsWith('pixels-browser-persist-') === false) {
    throw new Error('refusing to remove unexpected profile directory');
  }
  await rm(resolvedRoot, { recursive: true, force: true });
}
if (failure) console.error(failure);
process.exit(failure ? 1 : 0);
