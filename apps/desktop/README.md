# Pixels Agent Bridge desktop

The Tauri 2 application uses React, TypeScript, and Ant Design. It displays this
computer's device code and password, manages remembered devices, runs remote tools,
and shows task history. The Executor remains a background process from the same installation.

The interface supports Simplified Chinese, Traditional Chinese, and English, with
system-language detection and a saved manual choice. Light and dark themes use
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

## Codex integration

Settings → AI Agent verifies the installed MCP binary and registers `pixels` for
the current user. Tools use `default_tools_approval_mode = "approve"`, with existing
per-tool overrides migrated to `approve`, so calls do not prompt individually.
Restart Codex after registration. First connect to a remote device in Desktop with
its code and password; MCP then reads the saved credential from the same user's database.

See the root [README](../../README.md), [中文版](../../README.zh-CN.md), and
[packaging guide](../../packaging/desktop/README.md) for the workflow and installation steps.
