// Smoke test: speak MCP to the bundled pab-mcp exactly like the plugin does.
import { spawn } from 'node:child_process'

const exe = new URL('../native/win32-x64/pab-mcp.exe', import.meta.url).pathname.replace(/^\//, '')
const child = spawn(exe, [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
let buf = ''
let seq = 0
const pending = new Map()

child.stdout.setEncoding('utf8')
child.stdout.on('data', (chunk) => {
  buf += chunk
  let i
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i).trim()
    buf = buf.slice(i + 1)
    if (!line) continue
    const msg = JSON.parse(line)
    if (msg.id == null) continue
    const p = pending.get(msg.id)
    if (!p) continue
    pending.delete(msg.id)
    msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result)
  }
})
child.stderr.setEncoding('utf8')
child.stderr.on('data', (d) => process.stderr.write('[stderr] ' + d))

function request(method, params, timeoutMs = 30000) {
  const id = ++seq
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(method + ' timeout')) }, timeoutMs)
    pending.set(id, {
      resolve: (v) => { clearTimeout(timer); resolve(v) },
      reject: (e) => { clearTimeout(timer); reject(e) },
    })
    child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
  })
}

try {
  const init = await request('initialize', {
    protocolVersion: '2024-11-05',
    capabilities: {},
    clientInfo: { name: 'smoke', version: '0.0.0' },
  })
  console.log('initialize:', JSON.stringify(init).slice(0, 160))
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')

  const tools = await request('tools/list', {})
  console.log('tools:', tools.tools.length, 'first:', tools.tools.slice(0, 5).map((t) => t.name).join(', '))

  const result = await request('tools/call', { name: 'pab_list_devices', arguments: {} }, 60000)
  const text = (result.content ?? []).filter((c) => c.type === 'text').map((c) => c.text).join('\n')
  console.log('pab_list_devices:', text.slice(0, 400))
} catch (err) {
  console.error('FAILED:', err.message)
  process.exitCode = 1
} finally {
  child.kill()
}
