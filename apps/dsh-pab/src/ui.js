/**
 * The panel UI, loaded at runtime by the thin client bundle (src/client.js).
 * Changing this file only needs a page reload, not a DeepSeek Harness restart.
 * It must not import bare specifiers: React arrives through the mount runtime.
 */
export function mount(ctx, rt) {
  const React = rt.React
  const h = rt.h
  const { useState, useEffect, useCallback, useRef } = React

    const PRODUCT = 'Pixels Agent Bridge'
    const TAB_ID = 'dsh-pab'
    const TAB_KIND = 'pixels'
    const BRAND = '#007276'
    const BRAND_SOFT = 'rgba(0,114,118,.16)'

    const S = {
      root: { display: 'flex', flexDirection: 'column', height: '100%', minHeight: 0, fontSize: 13, color: 'inherit' },
      header: {
        display: 'flex', alignItems: 'center', gap: 10, padding: '14px 14px 12px',
        borderBottom: '1px solid rgba(127,127,127,.16)',
      },
      title: { fontSize: 14, fontWeight: 600, letterSpacing: '.2px' },
      subtitle: { fontSize: 11.5, opacity: 0.55, marginTop: 1, wordBreak: 'break-all' },
      body: {
        flex: 1, minHeight: 0, overflow: 'auto', padding: '14px 14px 24px',
        display: 'flex', flexDirection: 'column', gap: 18,
      },
      label: { fontSize: 11, fontWeight: 600, letterSpacing: '.06em', opacity: 0.5, marginBottom: 8 },
      card: {
        borderRadius: 12, border: '1px solid rgba(127,127,127,.16)',
        background: 'rgba(127,127,127,.07)', padding: 12,
      },
      inputWrap: { position: 'relative', flex: 1, minWidth: 0 },
      input: {
        width: '100%', boxSizing: 'border-box', padding: '8px 10px', borderRadius: 9,
        border: '1px solid rgba(127,127,127,.22)', background: 'rgba(127,127,127,.08)',
        color: 'inherit', fontSize: 13, outline: 'none', fontFamily: 'inherit',
      },
      inputMono: { fontFamily: 'ui-monospace, SFMono-Regular, Consolas, monospace', letterSpacing: '.06em' },
      primary: {
        border: '1px solid ' + BRAND, background: BRAND, color: '#fff', borderRadius: 9,
        padding: '8px 12px', fontSize: 12.5, fontWeight: 500, cursor: 'pointer',
        display: 'inline-flex', alignItems: 'center', gap: 6, whiteSpace: 'nowrap',
      },
      ghost: {
        border: '1px solid rgba(127,127,127,.24)', background: 'transparent', color: 'inherit',
        borderRadius: 9, padding: '5px 10px', fontSize: 12, cursor: 'pointer',
      },
      iconBtn: {
        border: 'none', background: 'transparent', color: 'inherit', opacity: 0.55,
        cursor: 'pointer', padding: 2, display: 'inline-flex', borderRadius: 6,
      },
      grid: { display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(210px, 1fr))', gap: 10 },
      device: {
        border: '1px solid rgba(127,127,127,.16)', background: 'rgba(127,127,127,.06)',
        borderRadius: 12, padding: '11px 12px', display: 'flex', flexDirection: 'column', gap: 6,
        transition: 'border-color .12s ease, background .12s ease',
      },
      code: {
        fontFamily: 'ui-monospace, SFMono-Regular, Consolas, monospace',
        fontSize: 14.5, fontWeight: 600, letterSpacing: '.09em',
      },
      name: { fontSize: 12, opacity: 0.6, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
      error: {
        padding: '8px 10px', borderRadius: 9, fontSize: 12, background: 'rgba(229,83,75,.12)',
        border: '1px solid rgba(229,83,75,.35)', color: '#ff8a80', whiteSpace: 'pre-wrap',
      },
      hint: { fontSize: 11.5, opacity: 0.5, lineHeight: 1.5 },
      empty: { fontSize: 12.5, opacity: 0.55, padding: '18px 0', textAlign: 'center' },
    }

    const API_HEADERS = { 'content-type': 'application/json', 'x-pixels-client': '1' }

    function api(path, body) {
      return fetch('/x-pixels' + path, {
        method: body ? 'POST' : 'GET',
        credentials: 'same-origin',
        headers: API_HEADERS,
        body: body ? JSON.stringify(body) : undefined,
      }).then(async (r) => {
        if (r.status === 403) return { ok: false, error: '请求被拒绝（跨站或缺少请求头）' }
        if (r.status === 404) {
          return { ok: false, error: '接口不存在：页面比运行中的 Host 新，请重启 DeepSeek Harness 后再试' }
        }
        return r.json()
      })
    }

    /**
     * One shared modal for every "this device needs its password" case: the
     * device list, a new terminal tab and a session that lost its credentials
     * all open the same dialog and await the same promise.
     */
    let passwordRequest = null
    let passwordOverlay = null

    function finishPasswordDialog(result) {
      if (passwordOverlay) {
        passwordOverlay.remove()
        passwordOverlay = null
      }
      const pending = passwordRequest
      passwordRequest = null
      if (pending) pending.resolve(result || { ok: false })
    }

    function requestDevicePassword(code, name) {
      finishPasswordDialog({ ok: false })
      const promise = new Promise((resolve) => {
        passwordRequest = { code, name, resolve }
      })
      openPasswordDialog()
      return promise
    }

    function openPasswordDialog() {
      const request = passwordRequest
      if (!request) return

      const el = (tag, css, text) => {
        const node = document.createElement(tag)
        if (css) node.style.cssText = css
        if (text != null) node.textContent = text
        return node
      }

      const overlay = el('div', 'position:fixed;inset:0;z-index:2147483000;display:flex;align-items:center;'
        + 'justify-content:center;background:rgba(0,0,0,.5)')
      const card = el('div', 'width:min(380px,92vw);box-sizing:border-box;padding:16px;border-radius:12px;'
        + 'display:flex;flex-direction:column;gap:10px;background:#161a1f;color:#e6edf3;'
        + 'border:1px solid rgba(127,127,127,.28);box-shadow:0 24px 64px rgba(0,0,0,.55)')

      card.appendChild(el('div', 'font-size:14px;font-weight:600', '需要设备密码'))
      card.appendChild(el('div', 'font-size:11.5px;line-height:1.55;opacity:.7',
        `设备 ${groupCode(request.code)}${request.name ? `（${request.name}）` : ''} 的密码还没有保存在这台电脑上，或者已经失效。输入一次即可继续，密码只保存在本机。`))

      const input = el('input', 'width:100%;box-sizing:border-box;padding:8px 10px;border-radius:8px;'
        + 'border:1px solid rgba(127,127,127,.35);background:rgba(127,127,127,.12);color:inherit;'
        + 'font-size:12.5px;outline:none')
      input.type = 'password'
      input.placeholder = '设备密码（在对方 Pixels Agent Bridge 桌面端查看）'
      card.appendChild(input)

      const error = el('div', 'font-size:11.5px;color:#e5534b;min-height:0')
      card.appendChild(error)

      const actions = el('div', 'display:flex;justify-content:flex-end;gap:8px')
      const cancel = el('button', 'padding:7px 14px;border-radius:8px;font-size:12.5px;cursor:pointer;'
        + 'background:transparent;color:inherit;border:1px solid rgba(127,127,127,.35)', '取消')
      const save = el('button', 'padding:7px 14px;border-radius:8px;font-size:12.5px;cursor:pointer;'
        + 'background:#007276;color:#fff;border:1px solid #007276', '保存并连接')
      actions.appendChild(cancel)
      actions.appendChild(save)
      card.appendChild(actions)
      overlay.appendChild(card)
      document.body.appendChild(overlay)
      passwordOverlay = overlay

      const close = (result) => finishPasswordDialog(result)

      const submit = async () => {
        const password = input.value
        if (!password) { error.textContent = '请输入设备密码'; input.focus(); return }
        save.disabled = true
        save.textContent = '保存中…'
        error.textContent = ''
        try {
          const response = await api('/device', { code: request.code, password })
          if (!response.ok) {
            error.textContent = response.error || '密码不正确'
            save.disabled = false
            save.textContent = '保存并连接'
            input.focus()
            return
          }
          close({ ok: true })
        } catch (err) {
          error.textContent = String(err && err.message ? err.message : err)
          save.disabled = false
          save.textContent = '保存并连接'
        }
      }

      cancel.onclick = () => close({ ok: false })
      save.onclick = () => { submit() }
      input.onkeydown = (event) => {
        if (event.key === 'Enter') submit()
        if (event.key === 'Escape') close({ ok: false })
      }
      overlay.onmousedown = (event) => { if (event.target === overlay) close({ ok: false }) }
      setTimeout(() => input.focus(), 30)
    }

    function toolText(result) {
      const content = result && result.content
      if (!Array.isArray(content)) return ''
      return content.filter((c) => c && c.type === 'text').map((c) => c.text).join('\n')
    }

    function groupCode(code) {
      const digits = String(code || '').replace(/\D/g, '').slice(0, 9)
      return digits.replace(/(\d{3})(?=\d)/g, '$1 ')
    }

    /** MCP failures that mean "we need the device password" rather than a bug. */
    const PASSWORD_ERROR = /device password|password|credential|unauthori[sz]ed|forbidden|access denied|authentication failed|not authorized/i
    function looksLikePasswordError(text) {
      return PASSWORD_ERROR.test(String(text || ''))
    }

    /** Call one MCP tool through the plugin host; returns { ok, data | error }. */
    async function callTool(tool, args) {
      const r = await api('/call', { tool, args })
      if (!r.ok) return { ok: false, error: r.error || 'request failed' }
      if (r.result && r.result.isError) return { ok: false, error: toolText(r.result) || 'tool failed' }
      try { return { ok: true, data: JSON.parse(toolText(r.result)) } } catch { return { ok: false, error: 'unexpected payload' } }
    }

    // xterm.js ships as a vendor asset served by the host; load it once.
    let terminalAssets = null
    function loadTerminalAssets() {
      if (terminalAssets) return terminalAssets
      terminalAssets = new Promise((resolve, reject) => {
        try {
          const link = document.createElement('link')
          link.rel = 'stylesheet'
          link.href = '/x-pixels/assets/xterm.css'
          document.head.appendChild(link)
          const script = document.createElement('script')
          script.src = '/x-pixels/assets/xterm.js'
          script.onerror = () => reject(new Error('xterm.js 加载失败'))
          script.onload = () => {
            const fit = document.createElement('script')
            fit.src = '/x-pixels/assets/addon-fit.js'
            fit.onerror = () => reject(new Error('addon-fit 加载失败'))
            fit.onload = () => resolve()
            document.head.appendChild(fit)
          }
          document.head.appendChild(script)
        } catch (err) { reject(err) }
      })
      return terminalAssets
    }

    const TERMINAL_THEME = {
      background: 'rgba(0,0,0,0)',
      foreground: '#d7d7d7',
      cursor: '#0b9aa0',
      selectionBackground: 'rgba(0,114,118,.45)',
      black: '#1d1f21', red: '#e5534b', green: '#3fb950', yellow: '#d29922',
      blue: '#4493f8', magenta: '#bc8cff', cyan: '#39c5cf', white: '#d7d7d7',
      brightBlack: '#6e7681', brightRed: '#ff7b72', brightGreen: '#56d364',
      brightYellow: '#e3b341', brightBlue: '#79c0ff', brightMagenta: '#d2a8ff',
      brightCyan: '#56d4dd', brightWhite: '#f0f6fc',
    }

    // One bottom dock for the whole app; it holds any number of live terminal
    // tabs (different devices, or several shells on one device). Plain
    // subscribable object, no React context.
    const dockStore = { open: false, tabs: [], activeId: 0, seq: 0, tall: false, listeners: new Set() }
    function setDock(patch) {
      Object.assign(dockStore, patch)
      for (const listener of dockStore.listeners) listener()
    }
    function openDock(code, name, identity, label, fallback, os) {
      const tab = {
        id: ++dockStore.seq,
        code,
        name: name || '',
        identity,
        identityLabel: label || (identity === 'service' ? 'System' : '用户会话'),
        fallback: Boolean(fallback),
        os: os || '',
      }
      setDock({ open: true, tabs: dockStore.tabs.concat(tab), activeId: tab.id })
    }
    function activateTab(id) {
      setDock({ activeId: id })
    }
    function closeTab(id) {
      const tabs = dockStore.tabs.filter((tab) => tab.id !== id)
      const activeId = dockStore.activeId === id ? (tabs.length ? tabs[tabs.length - 1].id : 0) : dockStore.activeId
      setDock({ tabs, activeId, open: tabs.length > 0 })
    }
    function closeDock() {
      setDock({ open: false, tabs: [], activeId: 0 })
    }
    function useDock() {
      const [, bump] = useState(0)
      useEffect(() => {
        const listener = () => bump((n) => n + 1)
        dockStore.listeners.add(listener)
        return () => { dockStore.listeners.delete(listener) }
      }, [])
      return dockStore
    }

    function Logo(props) {
      const size = props.size || 22
      return h('svg', { width: size, height: size, viewBox: '0 0 100 100', 'aria-hidden': 'true', focusable: 'false' },
        h('path', {
          d: 'M50 26 L29 71 M50 26 L71 71 M29 71 L71 71',
          stroke: props.color || BRAND, strokeWidth: 13, strokeLinecap: 'round', fill: 'none',
        }),
        h('circle', { cx: 50, cy: 23, r: 12, fill: props.color || BRAND }),
        h('circle', { cx: 27, cy: 73, r: 12, fill: props.color || BRAND }),
        h('circle', { cx: 73, cy: 73, r: 12, fill: props.color || BRAND }),
      )
    }

    function osKind(os) {
      const value = String(os || '').toLowerCase()
      if (value.startsWith('win')) return 'windows'
      if (value.startsWith('mac') || value === 'darwin') return 'macos'
      if (value.startsWith('linux')) return 'linux'
      return 'other'
    }

    function OsIcon(props) {
      const size = props.size || 20
      const kind = osKind(props.os)
      const box = { width: size, height: size, flex: '0 0 auto', opacity: 0.75 }
      if (kind === 'windows') {
        return h('svg', { ...box, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
          h('path', { fill: 'currentColor', d: 'M3 5.5 10.2 4.5v6.6H3V5.5Zm8.4-1.2L21 3v8.1h-9.6V4.3ZM3 12.3h7.2v6.6L3 17.9v-5.6Zm8.4 0H21V21l-9.6-1.3v-7.4Z' }))
      }
      if (kind === 'macos') {
        return h('svg', { ...box, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
          h('path', {
            fill: 'currentColor',
            d: 'M12.152 6.896c-.948 0-2.415-1.078-3.96-1.04-2.04.027-3.91 1.183-4.961 3.014-2.117 3.675-.546 9.103 1.519 12.09 1.013 1.454 2.208 3.09 3.792 3.039 1.52-.065 2.09-.987 3.935-.987 1.831 0 2.35.987 3.96.948 1.637-.026 2.676-1.48 3.676-2.948 1.156-1.688 1.636-3.325 1.662-3.415-.039-.013-3.182-1.221-3.22-4.857-.026-3.04 2.48-4.494 2.597-4.559-1.429-2.09-3.623-2.324-4.39-2.376-2-.156-3.675 1.09-4.61 1.09zM15.53 3.83c.843-1.012 1.4-2.427 1.245-3.83-1.207.052-2.662.805-3.532 1.818-.78.896-1.454 2.338-1.273 3.714 1.338.104 2.715-.688 3.559-1.701',
          }))
      }
      if (kind === 'linux') {
        return h('svg', { ...box, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
          h('path', {
            fill: 'currentColor',
            d: 'M12.504 0c-.155 0-.315.008-.48.021-4.226.333-3.105 4.807-3.17 6.298-.076 1.092-.3 1.953-1.05 3.02-.885 1.051-2.127 2.75-2.716 4.521-.278.832-.41 1.684-.287 2.489a.424.424 0 00-.11.135c-.26.268-.45.6-.663.839-.199.199-.485.267-.797.4-.313.136-.658.269-.864.68-.09.189-.136.394-.132.602 0 .199.027.4.055.536.058.399.116.728.04.97-.249.68-.28 1.145-.106 1.484.174.334.535.47.94.601.81.2 1.91.135 2.774.6.926.466 1.866.67 2.616.47.526-.116.97-.464 1.208-.946.587-.003 1.23-.269 2.26-.334.699-.058 1.574.267 2.577.2.025.134.063.198.114.333l.003.003c.391.778 1.113 1.132 1.884 1.071.771-.06 1.592-.536 2.257-1.306.631-.765 1.683-1.084 2.378-1.503.348-.199.629-.469.649-.853.023-.4-.2-.811-.714-1.376v-.097l-.003-.003c-.17-.2-.25-.535-.338-.926-.085-.401-.182-.786-.492-1.046h-.003c-.059-.054-.123-.067-.188-.135a.357.357 0 00-.19-.064c.431-1.278.264-2.55-.173-3.694-.533-1.41-1.465-2.638-2.175-3.483-.796-1.005-1.576-1.957-1.56-3.368.026-2.152.236-6.133-3.544-6.139zm.529 3.405h.013c.213 0 .396.062.584.198.19.135.33.332.438.533.105.259.158.459.166.724 0-.02.006-.04.006-.06v.105a.086.086 0 01-.004-.021l-.004-.024a1.807 1.807 0 01-.15.706.953.953 0 01-.213.335.71.71 0 00-.088-.042c-.104-.045-.198-.064-.284-.133a1.312 1.312 0 00-.22-.066c.05-.06.146-.133.183-.198.053-.128.082-.264.088-.402v-.02a1.21 1.21 0 00-.061-.4c-.045-.134-.101-.2-.183-.333-.084-.066-.167-.132-.267-.132h-.016c-.093 0-.176.03-.262.132a.8.8 0 00-.205.334 1.18 1.18 0 00-.09.4v.019c.002.089.008.179.02.267-.193-.067-.438-.135-.607-.202a1.635 1.635 0 01-.018-.2v-.02a1.772 1.772 0 01.15-.768c.082-.22.232-.406.43-.533a.985.985 0 01.594-.2zm-2.962.059h.036c.142 0 .27.048.399.135.146.129.264.288.344.465.09.199.14.4.153.667v.004c.007.134.006.2-.002.266v.08c-.03.007-.056.018-.083.024-.152.055-.274.135-.393.2.012-.09.013-.18.003-.267v-.015c-.012-.133-.04-.2-.082-.333a.613.613 0 00-.166-.267.248.248 0 00-.183-.064h-.021c-.071.006-.13.04-.186.132a.552.552 0 00-.12.27.944.944 0 00-.023.33v.015c.012.135.037.2.08.334.046.134.098.2.166.268.01.009.02.018.034.024-.07.057-.117.07-.176.136a.304.304 0 01-.131.068 2.62 2.62 0 01-.275-.402 1.772 1.772 0 01-.155-.667 1.759 1.759 0 01.08-.668 1.43 1.43 0 01.283-.535c.128-.133.26-.2.418-.2zm1.37 1.706c.332 0 .733.065 1.216.399.293.2.523.269 1.052.468h.003c.255.136.405.266.478.399v-.131a.571.571 0 01.016.47c-.123.31-.516.643-1.063.842v.002c-.268.135-.501.333-.775.465-.276.135-.588.292-1.012.267a1.139 1.139 0 01-.448-.067 3.566 3.566 0 01-.322-.198c-.195-.135-.363-.332-.612-.465v-.005h-.005c-.4-.246-.616-.512-.686-.71-.07-.268-.005-.47.193-.6.224-.135.38-.271.483-.336.104-.074.143-.102.176-.131h.002v-.003c.169-.202.436-.47.839-.601.139-.036.294-.065.466-.065zm2.8 2.142c.358 1.417 1.196 3.475 1.735 4.473.286.534.855 1.659 1.102 3.024.156-.005.33.018.513.064.646-1.671-.546-3.467-1.089-3.966-.22-.2-.232-.335-.123-.335.59.534 1.365 1.572 1.646 2.757.13.535.16 1.104.021 1.67.067.028.135.06.205.067 1.032.534 1.413.938 1.23 1.537v-.043c-.06-.003-.12 0-.18 0h-.016c.151-.467-.182-.825-1.065-1.224-.915-.4-1.646-.336-1.77.465-.008.043-.013.066-.018.135-.068.023-.139.053-.209.064-.43.268-.662.669-.793 1.187-.13.533-.17 1.156-.205 1.869v.003c-.02.334-.17.838-.319 1.35-1.5 1.072-3.58 1.538-5.348.334a2.645 2.645 0 00-.402-.533 1.45 1.45 0 00-.275-.333c.182 0 .338-.03.465-.067a.615.615 0 00.314-.334c.108-.267 0-.697-.345-1.163-.345-.467-.931-.995-1.788-1.521-.63-.4-.986-.87-1.15-1.396-.165-.534-.143-1.085-.015-1.645.245-1.07.873-2.11 1.274-2.763.107-.065.037.135-.408.974-.396.751-1.14 2.497-.122 3.854a8.123 8.123 0 01.647-2.876c.564-1.278 1.743-3.504 1.836-5.268.048.036.217.135.289.202.218.133.38.333.59.465.21.201.477.335.876.335.039.003.075.006.11.006.412 0 .73-.134.997-.268.29-.134.52-.334.74-.4h.005c.467-.135.835-.402 1.044-.7zm2.185 8.958c.037.6.343 1.245.882 1.377.588.134 1.434-.333 1.791-.765l.211-.01c.315-.007.577.01.847.268l.003.003c.208.199.305.53.391.876.085.4.154.78.409 1.066.486.527.645.906.636 1.14l.003-.007v.018l-.003-.012c-.015.262-.185.396-.498.595-.63.401-1.746.712-2.457 1.57-.618.737-1.37 1.14-2.036 1.191-.664.053-1.237-.2-1.574-.898l-.005-.003c-.21-.4-.12-1.025.056-1.69.176-.668.428-1.344.463-1.897.037-.714.076-1.335.195-1.814.12-.465.308-.797.641-.984l.045-.022zm-10.814.049h.01c.053 0 .105.005.157.014.376.055.706.333 1.023.752l.91 1.664.003.003c.243.533.754 1.064 1.189 1.637.434.598.77 1.131.729 1.57v.006c-.057.744-.48 1.148-1.125 1.294-.645.135-1.52.002-2.395-.464-.968-.536-2.118-.469-2.857-.602-.369-.066-.61-.2-.723-.4-.11-.2-.113-.602.123-1.23v-.004l.002-.003c.117-.334.03-.752-.027-1.118-.055-.401-.083-.71.043-.94.16-.334.396-.4.69-.533.294-.135.64-.202.915-.47h.002v-.002c.256-.268.445-.601.668-.838.19-.201.38-.336.663-.336zm7.159-9.074c-.435.201-.945.535-1.488.535-.542 0-.97-.267-1.28-.466-.154-.134-.28-.268-.373-.335-.164-.134-.144-.333-.074-.333.109.016.129.134.199.2.096.066.215.2.36.333.292.2.68.467 1.167.467.485 0 1.053-.267 1.398-.466.195-.135.445-.334.648-.467.156-.136.149-.267.279-.267.128.016.034.134-.147.332a8.097 8.097 0 01-.69.468zm-1.082-1.583V5.64c-.006-.02.013-.042.029-.05.074-.043.18-.027.26.004.063 0 .16.067.15.135-.006.049-.085.066-.135.066-.055 0-.092-.043-.141-.068-.052-.018-.146-.008-.163-.065zm-.551 0c-.02.058-.113.049-.166.066-.047.025-.086.068-.14.068-.05 0-.13-.02-.136-.068-.01-.066.088-.133.15-.133.08-.031.184-.047.259-.005.019.009.036.03.03.05v.02h.003z',
          }))
      }
      return h('svg', { ...box, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
        h('circle', { cx: 12, cy: 12, r: 8, fill: 'none', stroke: 'currentColor', strokeWidth: 1.6 }))
    }

    function Eye(props) {
      const open = props.open
      return h('svg', { width: 16, height: 16, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
        h('path', {
          d: open
            ? 'M2 12s3.6-6 10-6 10 6 10 6-3.6 6-10 6S2 12 2 12Z'
            : 'M3 3l18 18M10.6 6.2A9.6 9.6 0 0 1 12 6c6.4 0 10 6 10 6a17 17 0 0 1-3.2 3.6M6.2 7.3A17.4 17.4 0 0 0 2 12s3.6 6 10 6a9.7 9.7 0 0 0 3.3-.6',
          fill: 'none', stroke: 'currentColor', strokeWidth: 1.6, strokeLinecap: 'round',
        }),
        open ? h('circle', { cx: 12, cy: 12, r: 2.6, fill: 'none', stroke: 'currentColor', strokeWidth: 1.6 }) : null)
    }

    function CopyIcon() {
      return h('svg', { width: 14, height: 14, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
        h('rect', { x: 9, y: 9, width: 11, height: 11, rx: 2, fill: 'none', stroke: 'currentColor', strokeWidth: 1.6 }),
        h('path', { d: 'M5 15V6a2 2 0 0 1 2-2h8', fill: 'none', stroke: 'currentColor', strokeWidth: 1.6 }))
    }

    function Status(props) {
      const online = props.online
      const known = online === true || online === false
      const color = online === true ? '#3fb950' : (online === false ? '#8b949e' : 'transparent')
      const label = online === true ? '在线' : (online === false ? '离线' : '未知')
      return h('span', {
        style: { display: 'inline-flex', alignItems: 'center', gap: 5, flex: '0 0 auto', fontSize: 11, opacity: known ? 0.85 : 0.45 },
        title: known ? label : '未获取到在线状态',
      },
        h('span', {
          style: {
            width: 7, height: 7, borderRadius: '50%', background: color, boxSizing: 'border-box',
            border: known ? 'none' : '1px solid rgba(127,127,127,.55)',
          },
        }),
        label)
    }

    function DeviceCard(props) {
      const [hover, setHover] = useState(false)
      const [copied, setCopied] = useState(false)
      const device = props.device
      const code = String(device.device_code || '')

      const copy = async (event) => {
        event.stopPropagation()
        try {
          await navigator.clipboard.writeText(code)
          setCopied(true)
          setTimeout(() => setCopied(false), 1200)
        } catch { /* clipboard unavailable */ }
      }

      return h('div', {
        style: {
          ...S.device,
          cursor: 'pointer',
          borderColor: props.selected
            ? 'rgba(0,114,118,.75)'
            : (hover ? 'rgba(0,114,118,.45)' : 'rgba(127,127,127,.16)'),
          background: props.selected
            ? 'rgba(0,114,118,.14)'
            : (hover ? 'rgba(0,114,118,.08)' : 'rgba(127,127,127,.06)'),
        },
        onMouseEnter: () => setHover(true),
        onMouseLeave: () => setHover(false),
        onClick: props.onSelect,
        onContextMenu: props.onMenu,
        title: device.name || code,
      },
        h('div', { style: { display: 'flex', alignItems: 'center', gap: 8 } },
          h(OsIcon, { os: device.os }),
          h('div', { style: { ...S.code, flex: 1, minWidth: 0 } }, groupCode(code)),
          h('button', { style: S.iconBtn, onClick: copy, title: copied ? '已复制' : '复制设备码' },
            copied ? h('span', { style: { fontSize: 11 } }, '✓') : h(CopyIcon))),
        h('div', { style: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 } },
          h('div', { style: S.name }, device.name || '未命名设备'),
          h('div', { style: { display: 'flex', alignItems: 'center', gap: 6, flex: '0 0 auto' } },
            device.paired === false
              ? h('span', {
                  style: {
                    fontSize: 10.5, color: '#d29922', border: '1px solid rgba(210,153,34,.45)',
                    background: 'rgba(210,153,34,.10)', padding: '1px 7px', borderRadius: 999,
                  },
                }, '未配对')
              : null,
            h(Status, { online: device.online }))))
    }

    /** One live xterm bound to one device identity. Owns the PTY session. */
    function TerminalView(props) {
      const hostRef = useRef(null)
      const termRef = useRef(null)
      const fitRef = useRef(null)
      const sessionRef = useRef(null)
      const pollRef = useRef(null)
      const inputRef = useRef({ buffer: '', timer: null })
      const [error, setError] = useState('')
      const [hideHint, setHideHint] = useState(false)
      // Service/root terminals are not subject to the user-session disk-access
      // prompt, so the macOS notice only belongs to a genuine user shell.
      const [serviceTerminal, setServiceTerminal] = useState(props.identity === 'service')
      // Bumped after the shared password dialog stores a password, which re-runs
      // the open effect below.
      const [retryKey, setRetryKey] = useState(0)

      // A hidden pane has no measurable box, so the fit addon is re-run when the
      // tab becomes visible again.
      useEffect(() => {
        if (!props.visible) return undefined
        const timer = setTimeout(() => {
          try { fitRef.current?.fit() } catch { /* not laid out */ }
        }, 30)
        return () => clearTimeout(timer)
      }, [props.visible])

      useEffect(() => {
        let disposed = false
        const stopPolling = () => {
          if (pollRef.current) { clearInterval(pollRef.current); pollRef.current = null }
        }
        const flush = () => {
          const data = inputRef.current.buffer
          inputRef.current.buffer = ''
          inputRef.current.timer = null
          const id = sessionRef.current
          if (data && id) api('/call', { tool: 'pab_terminal_input', args: { session_id: id, data } }).catch(() => {})
        }
        const run = async () => {
          await loadTerminalAssets()
          if (disposed || !hostRef.current) return
          const term = new window.Terminal({
            fontSize: 12.5,
            fontFamily: 'ui-monospace, SFMono-Regular, Consolas, monospace',
            cursorBlink: true,
            scrollback: 3000,
            theme: TERMINAL_THEME,
          })
          const fit = new window.FitAddon.FitAddon()
          term.loadAddon(fit)
          term.open(hostRef.current)
          try { fit.fit() } catch { /* layout not settled */ }
          termRef.current = term
          fitRef.current = fit

          const attempts = props.fallback ? [props.identity, 'service'] : [props.identity]

          // A device whose password this computer never stored cannot open a
          // terminal at all; ask for it before spending a failed round-trip.
          try {
            const saved = await callTool('pab_list_saved_devices', {})
            const entry = saved.ok ? (saved.data.devices || []).find((item) => item.device_code === props.code) : null
            if (entry && entry.paired === false) {
              term.write('\u001b[90m[这台设备还没有保存密码，请在弹窗中输入一次]\u001b[0m\r\n')
              const result = await requestDevicePassword(props.code, props.name)
              if (result.ok && !disposed) setTimeout(() => setRetryKey((value) => value + 1), 900)
              return
            }
          } catch { /* listing unavailable: fall through to the open attempt */ }

          let info = null
          let usedIdentity = props.identity
          let lastError = ''
          for (const identity of attempts) {
            const execution = identity === 'service'
              ? { mode: 'service' }
              : { mode: 'user', context_ref: identity }
            const opened = await api('/call', {
              tool: 'pab_open_terminal',
              args: { device_code: props.code, rows: term.rows, cols: term.cols, execution },
            })
            if (disposed) { term.dispose(); return }
            if (!opened.ok) { lastError = opened.error || '打开终端失败'; continue }
            if (opened.result && opened.result.isError) { lastError = toolText(opened.result) || '打开终端失败'; continue }
            info = JSON.parse(toolText(opened.result))
            usedIdentity = identity
            break
          }
          if (!info) {
            if (looksLikePasswordError(lastError)) {
              term.write('\u001b[90m[需要设备密码，请在弹窗中输入]\u001b[0m\r\n')
              const result = await requestDevicePassword(props.code, props.name)
              if (result.ok && !disposed) setTimeout(() => setRetryKey((value) => value + 1), 900)
              return
            }
            setError(lastError || '打开终端失败')
            return
          }
          sessionRef.current = info.session_id
          setServiceTerminal(usedIdentity === 'service')
          const who = info.execution_identity?.account_name
          term.write(`\u001b[90m${info.shell || 'shell'}${who ? ' · ' + who : ''}\u001b[0m\r\n`)
          if (usedIdentity === 'service' && props.identity !== 'service') {
            term.write('\u001b[90m[该设备没有可用的登录用户会话，已改用服务账户]\u001b[0m\r\n')
          }

          term.onData((data) => {
            inputRef.current.buffer += data
            if (!inputRef.current.timer) inputRef.current.timer = setTimeout(flush, 40)
          })
          term.onResize(({ cols, rows }) => {
            const id = sessionRef.current
            if (id) api('/call', { tool: 'pab_terminal_resize', args: { session_id: id, cols, rows } }).catch(() => {})
          })

          pollRef.current = setInterval(async () => {
            const id = sessionRef.current
            if (!id) return
            const out = await api('/call', { tool: 'pab_terminal_read', args: { session_id: id } }).catch(() => null)
            if (!out) return
            if (!out.ok) {
              stopPolling()
              if (looksLikePasswordError(out.error)) {
                term.write('\u001b[90m[需要设备密码，请在弹窗中输入]\u001b[0m\r\n')
                const result = await requestDevicePassword(props.code, props.name)
                if (result.ok && !disposed) setTimeout(() => setRetryKey((value) => value + 1), 900)
                return
              }
              term.write(`\r\n\u001b[91m[read failed] ${out.error || ''}\u001b[0m\r\n`)
              return
            }
            if (out.result && out.result.isError) {
              stopPolling()
              const text = toolText(out.result) || 'unknown error'
              if (looksLikePasswordError(text)) {
                term.write('\u001b[90m[需要设备密码，请在弹窗中输入]\u001b[0m\r\n')
                const result = await requestDevicePassword(props.code, props.name)
                if (result.ok && !disposed) setTimeout(() => setRetryKey((value) => value + 1), 900)
                return
              }
              term.write(`\r\n\u001b[91m[read failed] ${text}\u001b[0m\r\n`)
              return
            }
            try {
              const chunk = JSON.parse(toolText(out.result))
              if (chunk.text) term.write(chunk.text)
              if (chunk.ended) stopPolling()
            } catch { /* transient */ }
          }, 300)
        }
        run().catch((e) => setError(String(e && e.message ? e.message : e)))

        return () => {
          disposed = true
          stopPolling()
          const id = sessionRef.current
          sessionRef.current = null
          try { termRef.current?.dispose() } catch { /* disposed */ }
          termRef.current = null
          fitRef.current = null
          if (id) api('/call', { tool: 'pab_terminal_close', args: { session_id: id } }).catch(() => {})
        }
      }, [props.code, props.identity, retryKey])

      return h('div', { style: { display: 'flex', flexDirection: 'column', gap: 6, minHeight: 0, flex: 1, height: '100%' } },
        error ? h('div', { style: S.error }, error) : null,
        (osKind(props.os) === 'macos' && !serviceTerminal && !hideHint) ? h('div', {
          style: {
            display: 'flex', gap: 8, alignItems: 'flex-start', padding: '9px 10px', borderRadius: 8,
            background: 'rgba(68,147,248,.10)', border: '1px solid rgba(68,147,248,.32)',
            fontSize: 11.5, lineHeight: 1.5,
          },
        },
          h('span', { style: { color: '#79c0ff', fontWeight: 700, lineHeight: '16px' } }, 'i'),
          h('div', { style: { flex: 1, minWidth: 0 } },
            h('div', { style: { fontWeight: 600, marginBottom: 2 } }, 'macOS 文件访问权限'),
            h('div', { style: { opacity: 0.8 } },
              '首次访问「下载 / 文稿 / 桌面」时，macOS 会在远端等待授权，命令可能暂时没有输出。请在该 Mac 的「系统设置 → 隐私与安全性 → 完全磁盘访问」中添加并启用下方的程序；已有旧条目时先移除再重新添加。授权后回来继续即可，也可以用 Ctrl+C 中断等待。这只是权限说明，不代表命令卡死。'),
            h('div', { style: { marginTop: 4, display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' } },
              h('code', { style: { fontFamily: 'ui-monospace, Consolas, monospace', fontSize: 11, opacity: 0.9 } },
                '/Library/Application Support/PixelsAgentBridge/pab-executor'),
              h('button', {
                style: { ...S.ghost, padding: '1px 8px', fontSize: 11 },
                onClick: () => navigator.clipboard.writeText('/Library/Application Support/PixelsAgentBridge/pab-executor').catch(() => {}),
              }, '复制路径'))),
          h('button', { style: { ...S.iconBtn, opacity: 0.7 }, onClick: () => setHideHint(true), title: '关闭提示' }, '×'))
          : null,
        h('div', {
          ref: hostRef,
          style: { flex: 1, minHeight: 0, borderRadius: 8, background: 'rgba(0,0,0,.35)', padding: 4, overflow: 'hidden' },
        }))
    }

    function TerminalDock() {
      const dock = useDock()
      if (!dock.open || dock.tabs.length === 0) return null
      return h('div', {
        style: {
          display: 'flex', flexDirection: 'column',
          height: dock.tall ? '62vh' : 320,
          borderTop: '1px solid rgba(127,127,127,.2)',
          background: 'rgba(127,127,127,.04)',
        },
      },
        // Tab strip: one tab per live PTY, whatever its device or identity.
        h('div', {
          style: {
            display: 'flex', alignItems: 'center', gap: 6, padding: '6px 10px',
            borderBottom: '1px solid rgba(127,127,127,.14)', overflowX: 'auto', flex: '0 0 auto',
          },
        },
          h(Logo, { size: 14 }),
          dock.tabs.map((tab) => h('div', {
            key: tab.id,
            onClick: () => activateTab(tab.id),
            title: `${tab.name || groupCode(tab.code)} · ${groupCode(tab.code)} · ${tab.identityLabel}`,
            style: {
              display: 'inline-flex', alignItems: 'center', gap: 6, padding: '3px 8px', borderRadius: 7,
              fontSize: 11.5, cursor: 'pointer', whiteSpace: 'nowrap',
              background: tab.id === dock.activeId ? 'rgba(0,114,118,.2)' : 'transparent',
              border: '1px solid ' + (tab.id === dock.activeId ? 'rgba(0,114,118,.55)' : 'rgba(127,127,127,.2)'),
            },
          },
            h('span', null, tab.name || groupCode(tab.code)),
            h('span', { style: { opacity: 0.5, fontSize: 10.5 } }, tab.identityLabel === 'System' ? 'SYS' : 'USR'),
            h('span', {
              style: { opacity: 0.55, padding: '0 2px' },
              onClick: (e) => { e.stopPropagation(); closeTab(tab.id) },
              title: '关闭这个终端',
            }, '×'))),
          h('div', { style: { flex: 1 } }),
          h('button', { style: S.ghost, onClick: () => setDock({ tall: !dock.tall }) }, dock.tall ? '恢复高度' : '加高'),
          h('button', { style: S.ghost, onClick: closeDock }, '全部关闭')),
        // Every tab stays mounted so its PTY keeps running; only the active one
        // is shown, and it refits its terminal when it becomes visible.
        h('div', { style: { position: 'relative', flex: 1, minHeight: 0 } },
          dock.tabs.map((tab) => h('div', {
            key: tab.id,
            style: {
              position: 'absolute', inset: '6px 10px 10px',
              display: tab.id === dock.activeId ? 'flex' : 'none',
              flexDirection: 'column', minHeight: 0,
            },
          }, h(TerminalView, {
            code: tab.code,
            identity: tab.identity,
            fallback: tab.fallback,
            os: tab.os,
            visible: tab.id === dock.activeId,
          })))))
    }

    const STATE_TONE = {
      completed: '#3fb950', succeeded: '#3fb950', done: '#3fb950',
      running: '#d29922', pending: '#d29922', queued: '#d29922', starting: '#d29922',
      failed: '#e5534b', cancelled: '#8b949e', canceled: '#8b949e', ended: '#8b949e',
    }

    function fmtTime(ms) {
      if (!ms) return ''
      try { return new Date(ms).toLocaleString() } catch { return '' }
    }

    function Chevron(props) {
      return h('svg', { width: 12, height: 12, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
        h('path', {
          d: props.up ? 'M6 15l6-6 6 6' : 'M6 9l6 6 6-6',
          fill: 'none', stroke: 'currentColor', strokeWidth: 2, strokeLinecap: 'round', strokeLinejoin: 'round',
        }))
    }

    function RefreshIcon() {
      return h('svg', { width: 14, height: 14, viewBox: '0 0 24 24', 'aria-hidden': 'true' },
        h('path', {
          d: 'M20 11a8 8 0 1 0-2.35 5.65M20 4v7h-7',
          fill: 'none', stroke: 'currentColor', strokeWidth: 1.8, strokeLinecap: 'round', strokeLinejoin: 'round',
        }))
    }

    function HistoryRow(props) {
      const [open, setOpen] = useState(false)
      const [hover, setHover] = useState(false)
      const item = props.item
      const tone = STATE_TONE[String(item.stateLabel || '').toLowerCase()] || '#8b949e'
      return h('div', { style: { borderTop: '1px solid rgba(127,127,127,.10)' } },
        h('div', {
          style: {
            display: 'flex', gap: 10, alignItems: 'center', padding: '10px 12px', cursor: 'pointer',
            background: hover ? 'rgba(127,127,127,.07)' : 'transparent',
            transition: 'background .12s ease',
          },
          onMouseEnter: () => setHover(true),
          onMouseLeave: () => setHover(false),
          onClick: () => setOpen(!open),
        },
          h('div', { style: { flex: 1, minWidth: 0 } },
            h('div', {
              style: { fontSize: 12.5, fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' },
              title: item.title,
            }, item.title),
            h('div', { style: { fontSize: 11, opacity: 0.5, marginTop: 2 } },
              `${item.deviceCode} · ${fmtTime(item.at)}`)),
          h('span', {
            style: {
              fontSize: 10.5, color: tone, flex: '0 0 auto',
              border: '1px solid ' + tone + '55', background: tone + '14',
              padding: '1px 7px', borderRadius: 999,
            },
          }, item.stateLabel)),
        open ? h('div', {
          style: {
            padding: '2px 12px 10px 12px', display: 'grid', gridTemplateColumns: '84px 1fr',
            rowGap: 4, columnGap: 10, fontSize: 11.5,
          },
        },
          Object.entries(item.detail)
            .filter(([, value]) => value !== null && value !== undefined && value !== '')
            .map(([key, value]) => h(React.Fragment, { key },
              h('div', { style: { opacity: 0.5 } }, key),
              h('div', { style: { wordBreak: 'break-all', fontFamily: 'ui-monospace, Consolas, monospace' } }, String(value))))) : null)
    }

    function HistoryView(props) {
      const [deviceCode, setDeviceCode] = useState(
        props.selected || (props.devices[0] && props.devices[0].device_code) || '')
      const [pages, setPages] = useState([])
      const [page, setPage] = useState(0)
      const [loading, setLoading] = useState(false)
      const [error, setError] = useState('')

      const buildRows = (data) => {
        const tasks = (data.tasks || []).map((task) => ({
          key: `t:${task.id}`,
          at: task.started_at_unix_ms || 0,
          deviceCode,
          title: (task.summary || `${task.program || ''} ${(task.args || []).join(' ')}`.trim()) || '命令执行',
          stateLabel: task.complete ? '已完成' : (task.state || '进行中'),
          detail: {
            id: task.id,
            program: task.program,
            args: (task.args || []).join(' '),
            cwd: task.cwd,
            started: fmtTime(task.started_at_unix_ms),
            finished: fmtTime(task.finished_at_unix_ms),
          },
        }))
        const operations = (data.operations || []).map((op) => ({
          key: `o:${op.id}`,
          at: op.started_at_unix_ms || 0,
          deviceCode,
          title: `${op.kind || 'operation'}${op.direction ? ' · ' + op.direction : ''}${
            op.source ? ` ${op.source} → ${op.destination}` : ''}`,
          stateLabel: op.state || '',
          detail: {
            id: op.id,
            kind: op.kind,
            direction: op.direction,
            source: op.source,
            destination: op.destination,
            size: op.size,
            started: fmtTime(op.started_at_unix_ms),
            finished: fmtTime(op.finished_at_unix_ms),
            message: op.message,
          },
        }))
        return tasks.concat(operations).sort((a, b) => b.at - a.at)
      }

      const loadPage = async (index, cursorUsed) => {
        if (!deviceCode) return
        setLoading(true); setError('')
        try {
          const args = { device_code: deviceCode, limit: 30 }
          if (cursorUsed && cursorUsed.task) args.task_before = cursorUsed.task
          if (cursorUsed && cursorUsed.operation) {
            args.operation_before_started_at_unix_ms = cursorUsed.operation.at
            args.operation_before_id = cursorUsed.operation.id
          }
          const res = await callTool('pab_list_history', args)
          if (!res.ok) { setError(res.error); return }
          const data = res.data || {}
          const rows = buildRows(data)
          const tasks = data.tasks || []
          const operations = data.operations || []
          const next = {
            task: tasks.length ? tasks[tasks.length - 1].id : null,
            operation: operations.length
              ? { at: operations[operations.length - 1].started_at_unix_ms, id: operations[operations.length - 1].id }
              : null,
          }
          const entry = { items: rows, cursorUsed: cursorUsed || null, next, total: data.total || rows.length }
          setPages((current) => {
            const copy = current.slice()
            copy[index] = entry
            return copy
          })
          setPage(index)
        } finally { setLoading(false) }
      }

      useEffect(() => {
        setPages([])
        setPage(0)
        loadPage(0, null).catch((e) => setError(String(e)))
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [deviceCode])

      const current = pages[page]
      const items = current ? current.items : []
      const total = current ? current.total : 0

      const goNext = () => {
        if (!current || !current.next) return
        loadPage(page + 1, current.next)
      }

      return h('div', { style: { display: 'flex', flexDirection: 'column', gap: 12, minHeight: 0 } },

        h('div', { style: { display: 'flex', gap: 8, alignItems: 'center' } },
          h('div', { style: { position: 'relative', flex: 1, minWidth: 0 } },
            h('select', {
              style: {
                ...S.input, paddingRight: 30, cursor: 'pointer',
                appearance: 'none', WebkitAppearance: 'none', MozAppearance: 'none',
              },
              value: deviceCode,
              onChange: (e) => setDeviceCode(e.target.value),
            }, props.devices.map((device) => h('option', { key: device.device_code, value: device.device_code },
              `${device.name || '未命名'} · ${groupCode(device.device_code)}`))),
            h('span', {
              style: {
                position: 'absolute', right: 10, top: '50%', transform: 'translateY(-50%)',
                pointerEvents: 'none', opacity: 0.55, display: 'inline-flex',
              },
            }, h(Chevron, { down: true }))),
          h('button', {
            style: { ...S.ghost, padding: '7px', display: 'inline-flex', alignItems: 'center', justifyContent: 'center' },
            disabled: loading,
            title: '刷新',
            onClick: () => loadPage(page, current ? current.cursorUsed : null),
          }, h(RefreshIcon))),

        error ? h('div', { style: S.error }, error) : null,

        h('div', {
          style: {
            border: '1px solid rgba(127,127,127,.16)', borderRadius: 10,
            overflow: 'hidden', background: 'rgba(127,127,127,.04)',
          },
        },
          items.length
            ? items.map((item) => h(HistoryRow, { key: item.key, item }))
            : h('div', { style: { ...S.empty, padding: '22px 0' } }, loading ? '加载中…' : '还没有记录')),

        h('div', { style: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 } },
          h('div', { style: S.hint }, total ? `共 ${total} 条` : ''),
          h('div', { style: { display: 'flex', alignItems: 'center', gap: 6 } },
            h('button', {
              style: { ...S.ghost, padding: '4px 8px', display: 'inline-flex', alignItems: 'center' },
              disabled: page === 0 || loading,
              title: '上一页',
              onClick: () => setPage(page - 1),
            }, h(Chevron, { up: true })),
            h('span', { style: { fontSize: 11.5, opacity: 0.7 } }, `第 ${page + 1} 页`),
            h('button', {
              style: { ...S.ghost, padding: '4px 8px', display: 'inline-flex', alignItems: 'center' },
              disabled: !current || !current.next || loading,
              title: '下一页',
              onClick: goNext,
            }, h(Chevron, { down: true })))))
    }

    /** Account avatar. Drawn as SVG so the shape is a true circle regardless of
     *  the host's global border-radius rules. */
    function Avatar(props) {
      const user = props.user || {}
      const size = props.size || 20
      const name = String(user.username || user.name || user.email || '').trim()
      const initial = name ? Array.from(name)[0].toUpperCase() : '?'
      const url = user.avatar_url || user.avatar || user.picture || null
      const center = size / 2
      const radius = center - 0.75
      const clipId = 'pab-avatar-clip'

      const shape = [
        h('circle', {
          key: 'bg',
          cx: center, cy: center, r: radius,
          fill: 'rgba(0,114,118,.22)', stroke: 'rgba(0,114,118,.45)', strokeWidth: 1.5,
        }),
      ]

      if (url) {
        shape.push(h('defs', { key: 'defs' },
          h('clipPath', { id: clipId }, h('circle', { cx: center, cy: center, r: radius }))))
        shape.push(h('image', {
          key: 'img',
          href: url,
          x: 0, y: 0, width: size, height: size,
          preserveAspectRatio: 'xMidYMid slice',
          clipPath: `url(#${clipId})`,
        }))
      } else {
        shape.push(h('text', {
          key: 'text',
          x: '50%', y: '50%',
          textAnchor: 'middle',
          dominantBaseline: 'central',
          fontSize: Math.round(size * 0.45),
          fontWeight: 600,
          fill: 'currentColor',
        }, initial))
      }

      return h('svg', {
        width: size, height: size, viewBox: `0 0 ${size} ${size}`,
        'aria-hidden': 'true', focusable: 'false',
        style: { flex: '0 0 auto', display: 'block' },
      }, shape)
    }

    function SettingsView(props) {
      const [controlUrl, setControlUrl] = useState('')
      const [relayUrls, setRelayUrls] = useState('')
      const [effective, setEffective] = useState(null)
      const [defaults, setDefaults] = useState(null)
      const [dataDir, setDataDir] = useState('')
      const [busy, setBusy] = useState(false)
      const [error, setError] = useState('')
      const [notice, setNotice] = useState('')

      const load = async () => {
        const r = await api('/settings')
        if (!r.ok) { setError(r.error || '读取设置失败'); return }
        setControlUrl(r.controlUrl || '')
        setRelayUrls(r.relayUrls || '')
        setEffective(r.effective || null)
        setDefaults(r.defaults || null)
        setDataDir(r.dataDir || '')
      }

      useEffect(() => { load().catch((e) => setError(String(e))) }, [])

      const save = async (next) => {
        setBusy(true); setError(''); setNotice('')
        try {
          const body = next || { controlUrl: controlUrl.trim(), relayUrls: relayUrls.trim() }
          const r = await api('/settings', body)
          if (!r.ok) { setError(r.error || '保存失败'); return }
          setEffective(r.effective || null)
          if (next) { setControlUrl(''); setRelayUrls('') }
          setNotice(r.signedOut ? '已保存并退出当前登录，请重新登录新服务' : '已保存')
          if (props && typeof props.onSaved === 'function') props.onSaved()
        } finally { setBusy(false) }
      }

      const field = (label, value, onChange, placeholder, hint) => h('div', { style: { display: 'flex', flexDirection: 'column', gap: 5 } },
        h('div', { style: { ...S.label, marginBottom: 0 } }, label),
        h('input', { style: { ...S.input, ...S.inputMono }, value, placeholder, onChange: (e) => onChange(e.target.value) }),
        hint ? h('div', { style: { fontSize: 11, opacity: 0.45 } }, hint) : null)

      // Both actions share one size so the row stays aligned.
      const buttonBase = {
        minWidth: 92, height: 34, boxSizing: 'border-box', padding: '0 14px',
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center', fontSize: 12.5,
      }

      return h('div', { style: { display: 'flex', flexDirection: 'column', gap: 12 } },
        h('div', { style: S.hint }, '留空表示使用官方服务；自托管时填入自己的控制服务器与 Relay。'),
        h('div', { style: { ...S.card, display: 'flex', flexDirection: 'column', gap: 10 } },
          field('控制服务器', controlUrl, setControlUrl, (defaults && defaults.controlUrl) || 'wss://pab.rgaa.vip/control', '必须为 wss:// 地址'),
          field('Relay 地址', relayUrls, setRelayUrls, (defaults && defaults.relayUrls) || 'https://pab-relay.rgaa.vip', '多个用英文逗号分隔，必须为 https:// 地址'),
          h('div', { style: { display: 'flex', gap: 8, alignItems: 'center', marginTop: 2, flexWrap: 'wrap' } },
            h('button', {
              style: { ...S.primary, ...buttonBase, opacity: busy ? 0.6 : 1 },
              disabled: busy,
              onClick: () => save(null),
            }, busy ? '验证并保存…' : '验证并保存'),
            h('button', {
              style: { ...S.ghost, ...buttonBase },
              disabled: busy,
              onClick: () => save({ controlUrl: '', relayUrls: '' }),
            }, '恢复默认'),
            notice ? h('span', { style: { fontSize: 11.5, color: '#3fb950' } }, notice) : null)),
        error ? h('div', { style: S.error }, error) : null,
        h('div', { style: { display: 'flex', flexDirection: 'column', gap: 4, fontSize: 11.5, opacity: 0.55 } },
          h('div', null, `当前生效：${(effective && effective.controlUrl) || '-'}`),
          h('div', null, `Relay：${(effective && effective.relayUrls) || '-'}`),
          dataDir ? h('div', { style: { wordBreak: 'break-all' } }, `独立数据目录：${dataDir}`) : null),
        h('div', { style: S.hint }, '插件使用自己的数据目录与配置，与已安装的桌面端互不影响；保存前会先验证服务可达性，切换服务器会退出当前登录并重启本地 MCP。'))
    }

    function PixelsPanel() {
      const [status, setStatus] = useState(null)
      const [devices, setDevices] = useState([])
      const [busy, setBusy] = useState(false)
      const [error, setError] = useState('')
      const [form, setForm] = useState({ username: '', password: '', register: false })
      const [code, setCode] = useState('')
      const [password, setPassword] = useState('')
      const [showPassword, setShowPassword] = useState(false)
      const [selected, setSelected] = useState('')
      const [menu, setMenu] = useState(null)
      const [view, setView] = useState('devices')

      const refresh = useCallback(async () => {
        const s = await api('/status')
        setStatus(s)
        if (!s.signedIn) { setDevices([]); return }
        const d = await api('/presence')
        if (!d.ok) { setError(d.error || '读取设备失败'); return }
        setDevices(Array.isArray(d.devices) ? d.devices : [])
      }, [])

      useEffect(() => { refresh().catch((e) => setError(String(e))) }, [refresh])

      const guard = async (fn) => {
        setBusy(true); setError('')
        try { await fn() } catch (e) { setError(String(e && e.message ? e.message : e)) } finally { setBusy(false) }
      }

      // The desktop app's "远程终端" is not "any user": it finds the single
      // active local console session, then matches that session's account to a
      // user execution context (uid on POSIX, session id + account on Windows).
      const resolveConsoleContext = async (deviceCode) => {
        const [sessionsRes, contextsRes] = await Promise.all([
          callTool('pab_list_sessions', { device_code: deviceCode, limit: 1000 }),
          callTool('pab_list_execution_contexts', { device_code: deviceCode, include_system: false, limit: 1000 }),
        ])
        const sessionsData = sessionsRes.ok ? (sessionsRes.data?.result?.data ?? {}) : {}
        const sessions = sessionsData.entries ?? []
        const backend = sessionsData.backend ?? ''
        const contexts = contextsRes.ok ? (contextsRes.data?.result?.data?.entries ?? []) : []
        const consoles = sessions.filter((s) => s.active === true && s.remote === false && s.user_name &&
          (s.session_type === 'console' || s.terminal === 'console' || (s.seat && String(s.seat).length > 0)))
        if (consoles.length !== 1) return null
        const consoleSession = consoles[0]
        const match = contexts.find((entry) => {
          if (entry.mode !== 'user' || entry.unavailable_reason) return false
          const identity = entry.identity || {}
          if (backend === 'windows_wts') {
            const name = consoleSession.domain ? `${consoleSession.domain}\\${consoleSession.user_name}` : consoleSession.user_name
            return identity.session_id === String(consoleSession.id) &&
              String(identity.account_name || '').toLowerCase() === String(name).toLowerCase()
          }
          return identity.account_id === `uid:${consoleSession.user_id}`
        })
        return match?.selection?.context_ref ?? null
      }

      const openMenu = (event, device) => {
        event.preventDefault()
        event.stopPropagation()
        setSelected(device.device_code)
        setMenu({
          code: device.device_code,
          name: device.name || device.device_code,
          os: device.os || device.os_reminder || '',
          x: event.clientX,
          y: event.clientY,
          loading: false,
        })
      }

      const menuItem = (label, onClick, disabled, key) => h('div', {
        key,
        style: { padding: '7px 10px', borderRadius: 7, fontSize: 12.5, cursor: disabled ? 'default' : 'pointer', opacity: disabled ? 0.45 : 1 },
        onMouseEnter: (e) => { if (!disabled) e.currentTarget.style.background = 'rgba(0,114,118,.18)' },
        onMouseLeave: (e) => { e.currentTarget.style.background = 'transparent' },
        onClick: disabled ? undefined : onClick,
      }, label)

      // "远程终端": mirror the desktop app — bind to the device's active console
      // account. When the device has no console login (headless server, some
      // macOS VMs), fall back to its service identity instead of failing.
      const openUserTerminal = async (m) => {
        setMenu(null)
        const contextRef = await resolveConsoleContext(m.code)
        if (!contextRef) {
          openDock(m.code, m.name, 'service', 'System', false, m.os)
          return
        }
        openDock(m.code, m.name, contextRef, '用户会话', true, m.os)
      }

      const signIn = () => guard(async () => {
        const r = await api('/login', { username: form.username.trim(), password: form.password, register: form.register })
        if (!r.ok) { setError(r.error || '登录失败'); return }
        setForm({ username: form.username, password: '', register: form.register })
        await refresh()
      })

      const signOut = () => guard(async () => {
        await api('/logout', {})
        setDevices([])
        closeDock()
        await refresh()
      })

      const addDevice = () => guard(async () => {
        const digits = code.replace(/\D/g, '')
        if (digits.length !== 9) { setError('设备码需要 9 位数字'); return }
        if (!password) { setError('请输入设备密码'); return }
        const r = await api('/device', { code: digits, password })
        if (!r.ok) { setError(r.error || '添加设备失败'); return }
        setCode(''); setPassword(''); setShowPassword(false)
        await refresh()
      })

      const signedIn = Boolean(status && status.signedIn)
      const user = status && status.user
      const displayName = (user && (user.username || user.name || user.email)) || '已登录'
      const digits = code.replace(/\D/g, '')
      const canAdd = digits.length === 9 && password.length > 0 && !busy
      const rank = (device) => (device.online === true ? 0 : (device.online === null ? 1 : 2))
      const orderedDevices = devices.slice().sort((left, right) => rank(left) - rank(right))

      const tabBar = h('div', {
        style: { display: 'flex', gap: 4, padding: 3, borderRadius: 9, background: 'rgba(127,127,127,.09)' },
      }, ['devices', 'history', 'settings'].map((key) => h('button', {
        key,
        onClick: () => setView(key),
        style: {
          flex: 1, padding: '5px 10px', borderRadius: 7, fontSize: 12, cursor: 'pointer',
          border: '1px solid ' + (view === key ? 'rgba(0,114,118,.5)' : 'transparent'),
          background: view === key ? 'rgba(0,114,118,.16)' : 'transparent',
          color: 'inherit',
        },
      }, key === 'devices' ? '设备' : (key === 'history' ? '任务记录' : '设置'))))

      return h('div', { style: S.root },

        h('div', { style: S.header },
          h(Avatar, { user, size: 36 }),
          h('div', {
            style: {
              flex: 1, minWidth: 0, fontSize: 18, fontWeight: 600, letterSpacing: '.2px',
              overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
            },
            title: displayName,
          }, signedIn ? displayName : '未登录'),
          signedIn ? h('button', { style: S.ghost, disabled: busy, onClick: signOut }, '退出') : null),

        error ? h('div', { style: { margin: '12px 14px 0' } }, h('div', { style: S.error }, error)) : null,

        !signedIn
          ? h('div', { style: S.body },
              tabBar,
              view === 'settings'
                ? h(SettingsView, { onSaved: () => refresh() })
                : h('div', { style: { ...S.card, padding: 16, display: 'flex', flexDirection: 'column', gap: 10 } },
                    h('div', { style: { display: 'flex', alignItems: 'center', gap: 8, marginBottom: 2 } },
                      h(Logo, { size: 22 }),
                      h('div', { style: { fontSize: 14, fontWeight: 600 } },
                        form.register ? `注册 ${PRODUCT} 账号` : `登录 ${PRODUCT}`)),
                    h('input', { style: S.input, placeholder: '用户名 / 邮箱', value: form.username, onChange: (e) => setForm({ ...form, username: e.target.value }) }),
                    h('input', { style: S.input, type: 'password', placeholder: '密码', value: form.password, onChange: (e) => setForm({ ...form, password: e.target.value }) }),
                    h('button', { style: { ...S.primary, justifyContent: 'center' }, disabled: busy, onClick: signIn }, form.register ? '注册并登录' : '登录'),
                    h('button', { style: { ...S.ghost, justifyContent: 'center' }, disabled: busy, onClick: () => setForm({ ...form, register: !form.register }) },
                      form.register ? '改用已有账号' : '注册新账号'),
                    h('div', { style: S.hint }, '登录状态保存在本机，下次自动登录')))
          : view === 'history'
            ? h('div', { style: S.body },
                tabBar,
                h(HistoryView, { devices, selected }))
            : view === 'settings'
              ? h('div', { style: S.body },
                  tabBar,
                  h(SettingsView, { onSaved: () => refresh() }))
              : h('div', { style: S.body },
                  tabBar,

              h('div', null,
                h('div', { style: S.label }, '添加设备'),
                h('div', { style: { ...S.card, display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' } },
                  h('div', { style: { ...S.inputWrap, flex: '1 1 150px' } },
                    h('input', {
                      style: { ...S.input, ...S.inputMono },
                      placeholder: '000 000 000',
                      inputMode: 'numeric',
                      value: groupCode(code),
                      onChange: (e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 9)),
                    })),
                  h('div', { style: { ...S.inputWrap, flex: '1 1 130px' } },
                    h('input', {
                      style: { ...S.input, paddingRight: 34 },
                      type: showPassword ? 'text' : 'password',
                      placeholder: '对方密码',
                      value: password,
                      onChange: (e) => setPassword(e.target.value),
                    }),
                    h('button', {
                      style: { ...S.iconBtn, position: 'absolute', right: 8, top: '50%', transform: 'translateY(-50%)' },
                      onClick: () => setShowPassword(!showPassword),
                      title: showPassword ? '隐藏密码' : '显示密码',
                    }, h(Eye, { open: showPassword }))),
                  h('button', { style: { ...S.primary, opacity: canAdd ? 1 : 0.55 }, disabled: !canAdd, onClick: addDevice },
                    '添加设备', h('span', { style: { fontSize: 15, lineHeight: 1 } }, '+'))),
                h('div', { style: { ...S.hint, marginTop: 8 } }, `在对方的 ${PRODUCT} 桌面端可以看到它的设备码和密码`)),

              h('div', null,
                h('div', { style: { display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 8 } },
                  h('div', { style: { ...S.label, marginBottom: 0 } }, `已添加的设备 · ${devices.length}`),
                  h('button', { style: S.ghost, disabled: busy, onClick: () => guard(refresh) }, '刷新')),
                devices.length === 0
                  ? h('div', { style: S.empty }, '还没有设备，先在上方添加一台')
                  : h('div', { style: S.grid }, orderedDevices.map((d) => h(DeviceCard, {
                      key: d.device_code,
                      device: d,
                      selected: selected === d.device_code,
                      onSelect: () => {
                        if (d.paired === false) {
                          requestDevicePassword(d.device_code, d.name).then((result) => {
                            if (result.ok) refresh().catch(() => {})
                          })
                        } else {
                          setSelected(d.device_code)
                        }
                      },
                      onMenu: (event) => openMenu(event, d),
                    }))))),

        menu ? h('div', {
          style: { position: 'fixed', inset: 0, zIndex: 2000 },
          onMouseDown: () => setMenu(null),
        },
          h('div', {
            style: {
              position: 'fixed',
              left: Math.min(menu.x, Math.max(8, window.innerWidth - 240)),
              top: Math.min(menu.y, Math.max(8, window.innerHeight - 230)),
              minWidth: 215, padding: 6, borderRadius: 10, display: 'flex', flexDirection: 'column', gap: 2,
              border: '1px solid rgba(127,127,127,.3)', background: 'rgba(30,32,36,.98)',
              boxShadow: '0 10px 30px rgba(0,0,0,.45)', color: '#e6e6e6',
            },
            onMouseDown: (e) => e.stopPropagation(),
          },
            h('div', { style: { padding: '4px 10px 6px', fontSize: 11, opacity: 0.5 } }, `${menu.name} · ${groupCode(menu.code)}`),
            menuItem('远程终端', () => openUserTerminal(menu), false, 'term-user'),
            menuItem('远程终端（System）', () => { openDock(menu.code, menu.name, 'service', 'System', false, menu.os); setMenu(null) }, false, 'term-system'),
            h('div', { style: { height: 1, margin: '4px 6px', background: 'rgba(127,127,127,.18)' } }),
            menuItem('复制设备码', () => { navigator.clipboard.writeText(menu.code).catch(() => {}); setMenu(null) }, false, 'copy')))
          : null
      )
    }

    function OpenButton(props) {
      const [hover, setHover] = useState(false)
      return h('button', {
        style: {
          display: 'inline-flex', alignItems: 'center', gap: 6,
          padding: '3px 10px 3px 7px', borderRadius: 7,
          border: '1px solid ' + (hover ? 'rgba(0,114,118,.55)' : 'rgba(127,127,127,.3)'),
          background: hover ? BRAND_SOFT : 'transparent',
          color: 'inherit', cursor: 'pointer', fontSize: 12.5, lineHeight: 1.5,
          transition: 'background .12s ease, border-color .12s ease',
        },
        onMouseEnter: () => setHover(true),
        onMouseLeave: () => setHover(false),
        onClick: () => props.open(),
      }, h(Logo, { size: 15 }), PRODUCT)
    }

    function report(message, stack) {
      try {
        console.warn('[dsh-pab]', message)
        fetch('/x-pixels/client-log', {
          method: 'POST',
          credentials: 'same-origin',
          headers: API_HEADERS,
          body: JSON.stringify({ message: String(message), stack: stack ? String(stack) : null }),
        }).catch(() => {})
      } catch { /* diagnostics must never break the UI */ }
    }

  try {
    ctx.effect(() => ctx.sidebarRightTabs.register({
      id: TAB_ID,
      kind: TAB_KIND,
      title: () => PRODUCT,
    }))
  } catch (err) {
    report('tab type registration failed: ' + (err && err.message), err && err.stack)
  }

  try {
    ctx.slots.inject('sidebar.right.pane.tab', () =>
      ctx.slots.register({ name: 'sidebar.right.pane.tab', key: TAB_ID }, PixelsPanel))
  } catch (err) {
    report('tab body registration failed: ' + (err && err.message), err && err.stack)
  }

  // Full-width terminal dock below the three columns.
  try {
    ctx.slots.inject('shell.bottom', () =>
      ctx.slots.register({ name: 'shell.bottom' }, TerminalDock))
  } catch (err) {
    report('shell.bottom registration failed: ' + (err && err.message), err && err.stack)
  }

  ctx.slots.inject('conversation.session.header.actions', () => {
    const open = () => {
      try {
        ctx.sidebarRight.openTab(TAB_KIND)
      } catch (err) {
        report('openTab failed: ' + (err && err.message), err && err.stack)
        window.alert(`打开 ${PRODUCT} 面板失败：` + (err && err.message))
      }
    }
    return ctx.slots.register(
      { name: 'conversation.session.header.actions', id: 'pixels-open', order: 90 },
      () => h(OpenButton, { open }))
  })
}
