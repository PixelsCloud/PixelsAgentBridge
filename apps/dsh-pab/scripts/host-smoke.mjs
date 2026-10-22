// Drive the host half without DSH: fake ctx/webServer, then exercise the routes.
import { apply } from '../src/index.js'

let route = null
let transform = null
const ctx = {
  effect(fn) { const d = fn(); return () => (typeof d === 'function' ? d() : undefined) },
  webServer: {
    register(r) { route = r; return () => {} },
    tapIndex(t) { transform = t; return () => {} },
  },
}

apply(ctx, {})

const call = async (path, method = 'GET', body) => {
  const payload = body ? Buffer.from(JSON.stringify(body)) : Buffer.alloc(0)
  const req = {
    url: '/x-pixels' + path,
    method,
    headers: { 'x-pixels-client': '1' },
    async *[Symbol.asyncIterator]() { yield payload },
  }
  let status = 0
  let text = ''
  const res = {
    writeHead(code) { status = code; return res },
    end(chunk) { text += chunk ?? ''; return res },
  }
  await route.handler(req, res)
  return { status, body: text }
}

for (let i = 0; i < 60; i++) {
  const h = JSON.parse((await call('/health')).body)
  if (h.state === 'ready' || h.state === 'failed') break
  await new Promise((r) => setTimeout(r, 500))
}

const check = await call('/selfcheck')
console.log('selfcheck', check.status, check.body.slice(0, 800))

const status = await call('/status')
console.log('status', status.status, status.body.slice(0, 300))

const devices = await call('/devices')
console.log('devices', devices.status, devices.body.slice(0, 160))

const presence = await call('/presence')
console.log('presence', presence.status, presence.body.slice(0, 220))

const css = await call('/assets/xterm.css')
console.log('asset css', css.status, 'bytes:', css.body.length)

const settings = await call('/settings')
console.log('settings', settings.status, settings.body.slice(0, 240))

const badSettings = await call('/settings', 'POST', { controlUrl: 'wss://127.0.0.1:9/control', relayUrls: '' })
console.log('settings verify', badSettings.status, badSettings.body.slice(0, 160))

const ui = await call('/ui.js')
console.log('ui.js', ui.status, 'bytes:', ui.body.length)

process.exit(0)
