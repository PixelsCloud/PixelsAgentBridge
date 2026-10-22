// Thin loader bundle: registers the plugin and pulls the actual UI module from
// the host at apply time. Editing src/ui.js therefore needs only a page reload;
// this file itself changes rarely (and a change here does need a restart).
window.__ModuleLoader__.load({
  id: 'dsh-pab',
  factory: (require) => {
    var module = { exports: {} }
    var exports = module.exports
    const React = require('react')

    exports.inject = ['slots', 'sidebarRightTabs', 'sidebarRight']

    exports.apply = function apply(ctx) {
      // The timestamp defeats the browser cache and the served module cache.
      const url = '/x-pixels/ui.js?v=' + Date.now()
      import(url)
        .then((mod) => {
          if (typeof mod.mount !== 'function') throw new Error('ui.js exports no mount()')
          return mod.mount(ctx, { React, h: React.createElement })
        })
        .catch((err) => {
          console.error('[dsh-pab] UI load failed', err)
          try {
            fetch('/x-pixels/client-log', {
              method: 'POST',
              credentials: 'same-origin',
              headers: { 'content-type': 'application/json', 'x-pixels-client': '1' },
              body: JSON.stringify({ message: 'ui load failed: ' + (err && err.message), stack: err && err.stack }),
            }).catch(() => {})
          } catch { /* diagnostics only */ }
        })
    }

    return module.exports
  }
})
