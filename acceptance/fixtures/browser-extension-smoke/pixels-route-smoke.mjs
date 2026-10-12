// Runs the public Pixels browser tools against the local-only fixture on a test device.
// Required environment: PAB_MCP_EXE, PAB_TEST_CODE, and the MCP account configuration.
import { spawn } from 'node:child_process';
import readline from 'node:readline';
import { randomUUID } from 'node:crypto';

const deviceCode = process.env.PAB_TEST_CODE;
if (!process.env.PAB_MCP_EXE || !/^\d{9}$/.test(deviceCode ?? '')) {
  throw new Error('PAB_MCP_EXE and PAB_TEST_CODE are required');
}

const child = spawn(process.env.PAB_MCP_EXE, [], {
  env: process.env, stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true,
});
let sequence = 0;
const pending = new Map();
readline.createInterface({ input: child.stdout }).on('line', line => {
  try {
    const response = JSON.parse(line);
    pending.get(response.id)?.(response);
    pending.delete(response.id);
  } catch { /* Ignore notifications. */ }
});
child.stderr.on('data', chunk => process.stderr.write(chunk));

function call(method, params) {
  const id = ++sequence;
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`${method} timed out`));
    }, 120000);
    pending.set(id, response => { clearTimeout(timeout); resolve(response); });
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
  });
}

function unpack(response) {
  if (response.error) throw new Error(JSON.stringify(response.error));
  if (response.result?.isError) throw new Error(JSON.stringify(response.result.content).slice(0, 600));
  const text = response.result?.content?.find(item => item.type === 'text')?.text;
  return response.result?.structuredContent ?? (text ? JSON.parse(text) : response.result);
}

async function tool(name, args) {
  let value = unpack(await call('tools/call', { name, arguments: args }));
  for (let attempt = 0; value.result?.state === 'running' && attempt < 40; attempt++) {
    const operationId = value.operation_ref?.operation_id;
    if (!operationId) throw new Error(`${name} has no operation ID`);
    await new Promise(resolve => setTimeout(resolve, 1000));
    value = unpack(await call('tools/call', {
      name: 'pab_get_operation',
      arguments: { device_code: deviceCode, operation_id: operationId, wait_ms: 1000, wait_until: 'complete' },
    }));
  }
  if (value.result?.state !== 'completed' && value.result?.state !== undefined) {
    throw new Error(`${name}: ${JSON.stringify(value.result?.error ?? value.result).slice(0, 600)}`);
  }
  return value;
}

function find(value, key) {
  if (!value || typeof value !== 'object') return undefined;
  if (typeof value[key] === 'string') return value[key];
  for (const item of Object.values(value)) {
    const found = find(item, key);
    if (found) return found;
  }
  return undefined;
}

let session;
let base;
try {
  await call('initialize', {
    protocolVersion: '2025-03-26', capabilities: {},
    clientInfo: { name: 'pixels-browser-route-smoke', version: '1' },
  });
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  const tools = unpack(await call('tools/list', {})).tools;
  if (!tools.some(item => item.name === 'pab_browser_attach')) throw new Error('browser tools missing');
  let connected = await tool('pab_connect', { device_code: deviceCode, wait_ms: 15000 });
  if (connected.connected === false) connected = await tool('pab_connect', { device_code: deviceCode, wait_ms: 15000 });
  if (connected.connected === false) throw new Error('test device did not connect');
  const contexts = await tool('pab_list_execution_contexts', { device_code: deviceCode });
  const contextRef = contexts.result?.data?.entries?.find(item => item.mode === 'desktop_user')?.selection?.context_ref;
  if (!contextRef) throw new Error('desktop_user context missing');
  base = { device_code: deviceCode, execution: { mode: 'desktop_user', context_ref: contextRef } };
  const attached = await tool('pab_browser_attach', { ...base, request_id: randomUUID() });
  session = find(attached.result?.data, 'session_ref');
  if (!session) throw new Error('attach returned no session');
  const mode = find(attached.result?.data, 'mode');
  if (mode && mode !== 'pixels') throw new Error(`unexpected mode ${mode}`);
  console.log('PIXELS_ATTACH_OK');
  const browser = { ...base, session_ref: session };
  await tool('pab_browser_navigate', { ...browser, url: 'http://127.0.0.1:17841/', request_id: randomUUID() });
  const before = await tool('pab_browser_snapshot', browser);
  if (!JSON.stringify(before).includes('Count: 0')) throw new Error('fixture not visible before click');
  await tool('pab_browser_click', { ...browser, target: '#increment', element: 'Increment', request_id: randomUUID() });
  const after = await tool('pab_browser_snapshot', browser);
  if (!JSON.stringify(after).includes('Count: 1')) throw new Error('click did not increment count');
  console.log('PIXELS_NAVIGATE_CLICK_OK');
  await tool('pab_browser_navigate', { ...browser, url: 'http://127.0.0.1:17841/account', request_id: randomUUID() });
  const account = await tool('pab_browser_snapshot', browser);
  if (JSON.stringify(account).includes('Sign in required')) {
    await tool('pab_browser_click', { ...browser, target: 'a[href="/demo-sign-in"]', element: 'Continue as demo (test only)', request_id: randomUUID() });
  } else if (!JSON.stringify(account).includes('Signed in as demo')) {
    throw new Error('test account page has neither signed-in nor signed-out state');
  }
  const signedIn = await tool('pab_browser_snapshot', browser);
  if (!JSON.stringify(signedIn).includes('Signed in as demo')) throw new Error('test-site login failed');
  await tool('pab_browser_close', { ...browser, request_id: randomUUID() });
  session = undefined;
  console.log('PIXELS_CLOSE_OK');
  const reopened = await tool('pab_browser_attach', { ...base, request_id: randomUUID() });
  session = find(reopened.result?.data, 'session_ref');
  if (!session) throw new Error('reopen returned no session');
  const reopenedBrowser = { ...base, session_ref: session };
  await tool('pab_browser_navigate', { ...reopenedBrowser, url: 'http://127.0.0.1:17841/account', request_id: randomUUID() });
  const retained = await tool('pab_browser_snapshot', reopenedBrowser);
  if (!JSON.stringify(retained).includes('Signed in as demo')) throw new Error('test-site login did not persist');
  await tool('pab_browser_close', { ...reopenedBrowser, request_id: randomUUID() });
  session = undefined;
  console.log('PIXELS_LOGIN_PERSISTENCE_OK');
} catch (error) {
  console.error('PIXELS_ROUTE_FAIL', String(error));
  process.exitCode = 1;
} finally {
  if (session && base) {
    try { await tool('pab_browser_close', { ...base, session_ref: session, request_id: randomUUID() }); }
    catch { /* Preserve the original error. */ }
  }
  child.kill();
}
