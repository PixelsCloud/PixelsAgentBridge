# Pixels Agent Bridge desktop

The Tauri 2 application uses React, TypeScript, and Ant Design. It displays this
computer's device code and password, manages remembered devices, runs remote tools,
and shows task history. Remote desktops and interactive terminals open in independent
windows. The Executor remains a background process from the same installation.

Windows and macOS installers include Desktop; Linux provides a separate Ubuntu
22.04 x86_64 graphical DEB alongside the headless package. See the
[first-version guide (Chinese)](../../docs/FIRST_VERSION.md) for user workflows.

The interface supports Simplified Chinese and English, with system-language
detection and a saved manual choice. Older Traditional Chinese preferences migrate
to Simplified Chinese. Light and dark themes use
separate palettes. Device details contain basic information, device-filtered history,
and remote tools; global and per-device task histories support pagination.

## Development

```sh
npm ci
npm run dev:desktop
```

This starts a native window with frontend hot reload. Restart after Rust edits because
the development command disables backend watching. Browser previews do not provide
Tauri's native APIs. Build a Windows Debug executable using the CLI directly:

```powershell
.\node_modules\.bin\tauri.cmd build --debug --no-bundle
```

The desktop Rust project is separate from the core workspace:

```sh
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
npm run build
```

## Local service connections

The Executor exposes an authenticated WebSocket on `127.0.0.1:7843` for local device
status and administration. Tokens stay in the machine and installing user's private
directories; the UI does not read the machine's endpoint key. `PAB_LOCAL_IPC_PORT`
can select another port.

The visible Desktop also runs an Axum reporting service on `0.0.0.0:26035`: HTTP
`/health` and `/api/mcp`, plus WebSocket `/ws/mcp`. This listener has no authentication
and is separate from the Executor IPC and MCP tool transport.

Each MCP process reports client identity, process/session, tool calls, device
connections, P2P/Relay paths, and task/transfer summaries. Settings → AI Agent shows
the live registry. Desktop and MCP retain independent operation runtimes; Desktop
does not proxy agent commands or file data. Reporting reconnects after Desktop reopens.

## AI agent integration

Desktop startup detects installed Codex, Kimi Code, Claude Code, DeepSeek Harness
and OpenCode clients, verifies the installed MCP binary, and registers `pixels`
for the current user. Missing clients are skipped. Existing unrelated settings
are preserved, conflicts are not overwritten, and already-correct entries are
not rewritten. Settings → AI Agent retains repair, enable and disable controls.
Explicit choices persist in the user's `agent-integrations.toml`; a disabled
integration stays disabled after restart. A newly installed client is detected
on the next Desktop launch.

Codex's Pixels tools use `default_tools_approval_mode = "approve"`, with existing
per-tool overrides migrated to `approve`. Start a new agent session or restart
the host after registration. First connect to a remote device in Desktop with
its code and password; MCP then reads the saved credential from the same user's
database. Linux headless installations register the MCP launcher manually.

## Remote desktop and human assistance

The device menu opens a separate remote desktop window in view-only mode.
Its title bar contains screen selection, 15/30 FPS limits, input control, text
clipboard, statistics and full-screen controls. Clipboard synchronization requires
input control and follows the active viewer. A target accepts one viewer at a time.
Capture, codecs, rendering and input use native workers; capability and login-screen
support depend on the target OS, helpers and permissions.

`pab_desktop_assistance` lets an agent request help in the operator's Desktop.
Opening or closing a window does not complete the request: the user explicitly
marks the assistance complete, and the agent must verify the task's result.
See [the tool workflow](../../docs/REMOTE_DESKTOP_ASSISTANCE.md).

See the root [README](../../README.md), [中文版](../../README.zh-CN.md), and
[packaging guide](../../packaging/desktop/README.md) for the workflow and installation steps.
