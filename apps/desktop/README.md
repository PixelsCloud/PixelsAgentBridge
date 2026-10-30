# Pixels Agent Bridge desktop

The Tauri 2 application uses React, TypeScript, and Ant Design. It displays this
computer's device code and password, manages remembered devices, runs remote tools,
and shows task history. Remote desktops, file transfers and interactive terminals open in independent
windows. The Executor remains a background process from the same installation.

Windows and macOS installers include Desktop; Linux provides a separate Ubuntu
22.04 x86_64 graphical DEB alongside the headless package. See the
[feature and usage guide (Chinese)](../../docs/FIRST_VERSION.md) for user workflows.

The interface supports Simplified Chinese and English, with system-language
detection and a saved manual choice. Older Traditional Chinese preferences migrate
to Simplified Chinese. Light and dark themes use
separate palettes. Device details contain basic information, device-filtered history,
and remote tools; global and per-device task histories support pagination.

## Development

```sh
npm ci
npm run dev:desktop -- --release
```

This starts a native window with frontend hot reload. Restart after Rust edits because
the development command disables backend watching. Browser previews do not provide
Tauri's native APIs. Build a Windows Release executable using the CLI directly:

```powershell
.\node_modules\.bin\tauri.cmd build --no-bundle
```

The desktop Rust project is separate from the core workspace:

```sh
cargo test --release --locked --manifest-path src-tauri/Cargo.toml --lib
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
Its title bar contains screen selection, 15/30/60 FPS limits (30 by default), input control,
text/PNG image clipboard sync, statistics and full-screen controls. FPS values are
requested limits, not guaranteed frame rates. Clipboard synchronization requires
input control and follows the active viewer. PNG compression is lossless and preserves
transparency; limits are 32 MiB of PNG data and 64 MiB of decoded pixel data. Files and
rich text are not transferred by this clipboard feature, and Android does not support it.
A target accepts one viewer at a time. An occupied session offers a confirmed takeover
when supported by the target; confirmation disconnects the previous viewer. Cancelling
preserves that session, and takeover does not bypass authentication or OS permissions.
The viewer uses the local OS pointer for immediate movement and applies the remote
cursor's shape and hotspot. Remote pointer positions do not move the local pointer.
Capture, codecs, rendering and input use native workers; capability and login-screen
support depend on the target OS, helpers and permissions.

## File-transfer and Android terminal windows

The device menu opens a separate two-pane file-transfer window with device tabs.
Browse the source and destination directories, select local items and choose Send,
or select remote items and choose Receive. Breadcrumbs, editable paths and the
location dropdown provide directory navigation; the toolbar contains search,
hidden-file visibility, refresh and file operations.

Transfers show progress, speed, elapsed time, counts and state. Available task
actions include pause, resume and cancel; Details filters and paginates individual
items. Name conflicts ask by default, with Replace, Skip and Keep both choices.
The apply-to-remaining checkbox affects only that transfer batch. Resolving the
last conflict closes the conflict view. Clear completed dismisses successfully
completed transfers, including skipped items; failed or unfinished transfers remain.
It does not delete transferred files.

Local browsing uses the Desktop user's identity. Remote computers require an
available signed-in desktop user; an unavailable user does not silently fall back
to the background service. Android uses the app UID and its authorized workspace
and shared directories. The remote heading shows P2P/Relay and available latency.

Android targets also support the independent terminal window with the app's own
UID. This is not root, ADB shell or a Windows System terminal. SAF-granted folders
are accessed through file tools, not as ordinary terminal working directories;
unavailable capabilities report their platform or permission limitation.

`pab_desktop_assistance` lets an agent request help in the operator's Desktop.
Opening or closing a window does not complete the request: the user explicitly
marks the assistance complete, and the agent must verify the task's result.
See [the tool workflow](../../docs/REMOTE_DESKTOP_ASSISTANCE.md).

See the root [README](../../README.md), [中文版](../../README.zh-CN.md), and
[packaging guide](../../packaging/desktop/README.md) for the workflow and installation steps.
