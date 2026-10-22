// List tool names from the bundled pab-mcp, filtered by a pattern.
import { spawn } from 'node:child_process'
import { homedir } from 'node:os'
import path from 'node:path'

const exe = process.env.PAB_MCP_PATH || path.join(
  process.env.LOCALAPPDATA || path.join(homedir(), 'AppData', 'Local'),
  'dsh-pab', 'bin', 'pab-mcp.exe',
)

const child = spawn(exe, [], {
  stdio: ['pipe', 'pipe', 'pipe'],
  windowsHide: true,
  env: { ...process.env, PAB_LOG_DIR: path.join(path.dirname(path.dirname(exe)), 'logs', 'tools-probe') },
})
let buf = ''
let seq = 0
const pending = new Map()
child.stdout.setEncoding('utf8')
child.stdout.on('data', (chunk) => {
  buf += chunk
  let i
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1)
    if (!line) continue
    const msg = JSON.parse(line)
    if (msg.id == null) continue
    const p = pending.get(msg.id); if (!p) continue
    pending.delete(msg.id)
    msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result)
  }
})
const request = (method, params) => new Promise((resolve, reject) => {
  const id = ++seq
  pending.set(id, { resolve, reject })
  child.stdin.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n')
})

await request('initialize', { protocolVersion: '2024-11-05', capabilities: {}, clientInfo: { name: 'probe', version: '0' } })
child.stdin.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n')
const { tools } = await request('tools/list', {})
const pattern = new RegExp(process.argv[2] || 'system|info|status', 'i')
console.log(tools.map((t) => t.name).filter((n) => pattern.test(n)).join('\n'))
child.kill()
process.exit(0)
