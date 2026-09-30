# Pixels Agent Bridge

**Control your devices remotely with your AI agent.**

English · [简体中文](README.zh-CN.md)

Pixels Agent Bridge connects your local AI agent to remote computers through the
Model Context Protocol (MCP). An agent can select a device, inspect its operating
system, run native commands, transfer files, use an interactive terminal, and work
with supported desktop capabilities. The desktop app provides device management,
task history, and a live view of the MCP processes operating your devices.

The project is in active development. Windows and Linux command and file workflows
have been exercised on real machines. Platform-specific support and validation
status are described below.

## Contents

- [Features](#features)
- [Workflow and components](#workflow)
- [Getting started](#getting-started)
- [MCP tool reference](#mcp-tool-reference)
- [Connections and activity](#connections-and-activity)
- [Common questions](#common-questions)
- [Platform status](#platform-status)
- [Development and builds](#development)
- [Self-hosting](#self-hosting)
- [Repository guide](#repository-guide)

## Features

- **Agent-driven operations:** select devices and perform remote operations through
  a consistent set of `pab_*` MCP tools. Codex integration is implemented.
- **Native commands:** execute a program with an explicit argument array, query its
  task state, and read stdout or stderr. Results include the verified target OS.
- **File transfer:** upload and download binary files using absolute paths, with
  integrity verification and explicit overwrite behavior.
- **Interactive terminals:** open, send input, read output, resize, and close remote
  terminal sessions.
- **Desktop capabilities:** enumerate windows, save remote screenshots as local PNG
  files, and send supported mouse, keyboard, or Windows secure attention events.
- **Device management:** remember and rename devices, copy their information, and
  distinguish online state from connection state. Connected cards show P2P or Relay.
- **Task history:** filter records by device, browse paginated results, and view a
  device's history alongside its information and remote tools.
- **MCP activity:** view live processes, client identities, device connections, tool
  calls, tasks, and transfer summaries in Settings.
- **Self-hosting:** deploy the control backend, PostgreSQL, and Relay using Docker Compose.

The desktop supports light and dark themes, Simplified Chinese, Traditional Chinese,
and English. It selects the system language on first launch and remembers your choice.

## Workflow

![Animated Pixels Agent Bridge workflow](diagram/workflow/agent-workflow.svg)

[Open the SVG](diagram/workflow/agent-workflow.svg) ·
[Static PNG](diagram/workflow/agent-workflow@2x.png)

Open the SVG in a browser to see the animated flow. Previews without animation
support still display the complete architecture.

1. **Request:** the AI client's MCP connection calls a Pixels tool through the local
   `pab-mcp` process. The current MCP transport is stdio.
2. **Authorize:** Bridge resolves the device and authenticates access. The control
   service provides identity, presence, authorized addresses, and Relay policy.
3. **Connect:** iroh establishes a device connection using P2P or Relay fallback.
   The path can change as network conditions change. The animation illustrates both
   routes; they are alternative paths.
4. **Execute:** the target Executor handles commands, files, terminals, and desktop
   operations in that device's native environment.
5. **Return:** the agent queries task state, progress, and output. Local task records
   and MCP status reports also make activity visible in Desktop.

**The control backend coordinates access; operation data travels through the device
connection, directly or through a Relay.** Each MCP process has its own Bridge
runtime and connections. Desktop receives status reports and reads task records;
it does not serve as a shared execution proxy for agents.

### Components

| Component | Responsibility |
|---|---|
| `pab-desktop` | Tauri desktop UI, device management, remote tools, task history, and MCP activity |
| `pab-mcp` | Local stdio MCP server exposing device tools to an AI agent |
| `pab-bridge` | Shared operator runtime, remote operations, and local persistence; also provides a development CLI |
| `pab-executor` | Background service on the target device; authenticates peers and executes operations |
| `pab-server` | TLS control backend for accounts, devices, endpoints, permissions, and Relay policy |
| `pab-relay-server` | Authorized Relay forwarding and configured bandwidth limits |
| PostgreSQL | Central account, device, membership, and authorization data |
| SQLite | Local device identity, credentials, task records, and operation history |

Windows installation also uses desktop session helpers for capabilities requiring
an interactive or secure desktop. These are separate from the visible UI window.

## Getting started

### 1. Install the complete package

Install the desktop package on the operator and target computers. The Windows
package contains Desktop, Executor, MCP, and installation scripts. The EXE installer
starts the background service and creates application shortcuts.

Installation requires a deployment UUID, a WSS control URL, and an HTTPS Relay URL.
Use the same deployment on computers that should communicate. For your own backend,
follow [Self-hosting](#self-hosting) first. Packages can be generated with the
[Windows build instructions](#build-a-windows-package).

Windows unattended access uses a machine service. The target's visible desktop
window does not have to stay open to accept connections.

### 2. Save a device connection

1. On the target, open **My Device** and obtain its nine-digit device code and password.
2. On the operator, enter the code and password in the connection form.
3. Connect once to save the device and credential in the local user's Bridge database.
4. Select the saved device to inspect its information, task history, or remote tools.
   Double-click a device item to connect.

Device codes are displayed with spaces for readability. MCP arguments and copied
IDs use nine digits without spaces, such as `123456789`.

### 3. Enable Codex

Install Codex CLI for the current OS user and make it available on `PATH`.
Open **Settings → AI Agent** in Desktop and enable Codex. This verifies the bundled
MCP executable, registers `pixels` for the current OS user, and configures tools to
run without individual approval prompts. Restart Codex to load the entry.

For manual Windows setup, use the following Codex configuration, adjusting the path:

```toml
[mcp_servers.pixels]
command = "C:\\Program Files\\PixelsAgentBridge\\pab-mcp.exe"
default_tools_approval_mode = "approve"
```

`approve` pre-approves tool calls. `auto` may still request approval based on tool
annotations. Remote device authentication and operating system permissions still apply.

On Linux and macOS, register the installed `run-mcp.sh` entry point so it loads the
deployment settings. Other MCP clients can use the stdio entry point; their automatic
setup and compatibility have not yet been validated to the same extent as Codex.

### 4. Ask the agent to operate a device

Example requests:

> Connect to device 123456789, confirm its operating system, and show free disk space
> on its system drive. Use Pixels tools and choose commands for the target OS.

> Upload C:\build\app.zip to C:\Users\Operator\Desktop\app.zip on device 123456789.
> Replace the existing file and verify its SHA-256 after uploading.

Replace the device code and paths with your own information.

## MCP tool reference

The current server exposes **16 tools**. Hosts may display them with a namespace,
for example `pixels.pab_connect`.

| Tool | Purpose |
|---|---|
| `pab_list_devices` | List locally remembered devices, including while the control server is offline |
| `pab_connect` | Authenticate a device and return verified OS and execution context |
| `pab_run_command` | Start a native executable with an explicit argument array |
| `pab_get_task` | Query state, progress, completion, and output ranges |
| `pab_read_output` | Read retained stdout or stderr by offset |
| `pab_list_directory` | List a page of directory entries |
| `pab_list_windows` | List windows in a supported desktop session |
| `pab_capture_screenshot` | Save a remote screenshot to a local PNG; return dimensions and hash |
| `pab_desktop_input` | Send supported mouse, keyboard, or secure attention events |
| `pab_open_terminal` | Open an interactive terminal |
| `pab_terminal_input` | Send terminal input |
| `pab_terminal_read` | Read terminal output |
| `pab_terminal_resize` | Resize a terminal |
| `pab_terminal_close` | Close a terminal session |
| `pab_upload_file` | Upload a binary file; optionally replace an existing regular file |
| `pab_download_file` | Download a binary file; optionally replace an existing regular file |

Call `pab_connect` first and retain its platform context. Most device tools require
`device_code`; terminal follow-up tools use the returned `session_id`. Passwords
come from the local Bridge database and are not tool arguments.

### Example: run a command and read the result

These JSON objects are tool arguments, not shell commands.

Call `pab_connect`:

```json
{ "device_code": "123456789" }
```

If the verified target is Windows, call `pab_run_command`:

```json
{
  "device_code": "123456789",
  "program": "powershell.exe",
  "args": ["-NoProfile", "-NonInteractive", "-Command", "Get-PSDrive -PSProvider FileSystem"]
}
```

For a Linux target, use `program: "df"` and `args: ["-h"]`. Bridge does not add an
implicit shell; select a shell explicitly when using shell syntax.

Query `pab_get_task` with `device_code` and the returned `task.task_ref.task_id`
until the task completes. Then read stdout with `pab_read_output`:

```json
{
  "device_code": "123456789",
  "task_id": "TASK_ID_FROM_THE_COMMAND_RESULT",
  "stream": "stdout",
  "offset": 0
}
```

File tools currently wait for the transfer to finish. Large transfers can encounter
the host's tool timeout; confirm the resulting state before retrying an operation.

## Connections and activity

Desktop and every MCP process maintain independent connections. Disconnecting a
device in Desktop does not, by itself, disconnect an agent's MCP connection.
Multiple agents also retain independent runtimes.

Desktop's main process exposes an Axum reporting service on `0.0.0.0:26035`:

| Endpoint | Purpose |
|---|---|
| `GET /health` | Reporting service health |
| `GET /api/mcp` | Current MCP process snapshots |
| `WS /ws/mcp` | MCP registration, heartbeats, and snapshots |

MCP processes normally connect to `ws://127.0.0.1:26035/ws/mcp`. Reports include client
identity, process/session information, device codes and names, connection phases,
P2P/Relay paths, and task or transfer summaries. Passwords, keys, raw command arguments,
and command output are omitted. Task details are persisted in SQLite.

This listener currently has no authentication. It is a reporting service, not an
HTTP MCP tool endpoint. Reporting reconnects when Desktop reopens; its availability
does not determine whether MCP can operate devices.

## Common questions

**What is the difference between online and connected?**

Online indicates that the device is present on the control service. Connected
indicates that this particular operator runtime has established and authenticated
a device connection. An online device may still be disconnected in Desktop.

**Why can an agent operate a device that Desktop shows as disconnected?**

`pab_connect` establishes or reuses that MCP process's connection using the saved
credential. Desktop's connection indicator belongs to Desktop's own runtime.
Check **Settings → AI Agent → MCP connections** to inspect agent activity.

**Can multiple agents operate the same computer?**

They use independent connections, task IDs, and terminal session IDs. Execution
still occurs on the same computer: concurrent edits to the same file or input to
the same desktop can interfere. Use distinct file destinations and coordinate
operations that share device resources.

**What should I do when the MCP executable exits unexpectedly?**

Restart the MCP connection in a host that supports it, or reopen the agent session.
Starting another standalone `pab-mcp` process does not reconnect an existing broken
stdio stream. Automatic host recovery remains under investigation.

## Platform status

| Area | Current status |
|---|---|
| Windows desktop, commands, files, and terminals | Implemented; core remote workflows verified on real machines |
| Windows screenshots and desktop input | Signed-in desktop verified; signed-out screenshots and secure attention also exercised |
| Linux commands and files | Verified on real remote Linux machines |
| Linux graphical desktop | Components implemented; graphical-machine validation remains; input requires X11 |
| macOS | Installation templates and platform code exist; no complete real-machine validation yet |
| Codex | One-click registration and actual tool calls verified |
| Other MCP hosts | Manual stdio entry point available; host-specific validation remains |

Continuous remote video streaming, a shared Bridge Host, and an HTTP MCP execution
endpoint are not part of the current implementation. Recovery from unexpected stdio
process exit depends on the host; automatic recovery is still under investigation.

## Development

### Requirements

- Rust **1.95.0**, selected by `rust-toolchain.toml`.
- Node.js **20.19+ in the 20.x line, or 22.12+**, as required by the pinned Vite, and npm.
- Python 3 for packaging and repository scripts.
- Platform-specific Tauri dependencies: MSVC build tools and WebView2 on Windows;
  Linux dependency examples are in `packaging/desktop/Dockerfile.linux`.
- Docker and Compose for self-hosting or the Linux build path.
- NSIS **3.12** under `tools/nsis` to produce a Windows EXE installer.

### Start the desktop development window

```sh
git clone git@github.com:PixelsCloud/PixelsAgentBridge.git
cd PixelsAgentBridge/apps/desktop
npm ci
npm run dev:desktop
```

The native Tauri window supports frontend hot reload. Rust watching is disabled by
this development command; restart after backend edits. A regular browser preview
does not provide Tauri's native device APIs.

### Build a Windows package

From the repository root, compile core Debug binaries:

```powershell
cargo build --locked -p pab-executor --bin pab-executor -p pab-bridge --bin pab-mcp
```

From `apps/desktop`, compile Desktop:

```powershell
npm ci
.\node_modules\.bin\tauri.cmd build --debug --no-bundle
```

Return to the repository root and generate the complete archive and installer:

```powershell
python packaging/desktop/build.py --platform windows --profile debug
python packaging/desktop/build_nsis.py --profile debug --deployment-id "YOUR_DEPLOYMENT_UUID" --control-url "wss://control.example.com/control" --relay-url "https://relay.example.com"
```

Replace the deployment values before running. Output is under `.build/packages/`,
including `pixels-agent-bridge-windows-x86_64-debug-setup.exe` and checksum manifests.
Use complete packages for installation and upgrade testing. Release packaging needs
the matching Release binaries and explicit profile selection.

### Checks

The desktop Rust project is separate from the core Cargo workspace:

```sh
python scripts/verify_iroh_vendor.py
cargo test --locked --workspace
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Run `npm run build` from `apps/desktop` for frontend validation. PostgreSQL integration
tests need a disposable test database through `DATABASE_URL`. Environment-dependent
and ignored tests require their documented setup. These are check commands, not a
claim that all platforms or optional tests have passed.

## Self-hosting

The supplied Compose stack runs PostgreSQL 17, the control backend, and Relay.

1. Copy `packaging/docker/example.env` to `packaging/docker/private.env`.
2. Set a deployment UUID and keep it unchanged for that deployment.
3. Configure the database password, DNS names, certificate paths, and port mappings.
4. Provision backend and Relay TLS certificates matching the DNS names, plus their
   CA files as required.
5. Create the private Relay control secret file specified by the environment file.

Run from the repository root:

```sh
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml build
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml ps
```

Backend and Relay HTTPS ports bind to loopback by default for a reverse proxy; Relay
QUIC publishes UDP 7842. PostgreSQL stays on the private Compose network, and its
named volume preserves data across ordinary container replacements.
`PAB_RELAY_IMAGE` can select a Relay image independently of the backend image.

See [Docker deployment](packaging/docker/README.md) for certificate filenames,
secrets, persistence, and configuration details.

## Repository guide

| Path | Contents |
|---|---|
| `apps/desktop` | React, TypeScript, Ant Design, and Tauri application |
| `crates/bridge` | Operator runtime, persistence, CLI, and MCP tools |
| `crates/executor` | Target service and remote capabilities |
| `crates/server` | Central control service and PostgreSQL data model |
| `crates/relay` | Relay and control-policy synchronization |
| `crates/agent-core`, `protocol`, `transport`, `task-runtime` | Shared identity, protocol, network, and task foundations |
| `crates/platform`, `terminal`, `windows-sas` | Native platform, terminal, and Windows secure attention integration |
| `packaging` | Desktop installers and Docker deployment |
| `patches/iroh-relay`, `vendor/iroh-relay` | Pinned Relay patch, verification metadata, and vendored source |
| `diagram/workflow` | Public SVG and PNG workflow assets |

Further reading: [Development](DEVELOPMENT.md), [desktop application](apps/desktop/README.md),
and [desktop packaging](packaging/desktop/README.md).

For issues and contributions, include the platform, whether the operation came from
Desktop or MCP, reproduction steps, and logs with credentials removed.
[Open an issue](https://github.com/PixelsCloud/PixelsAgentBridge/issues).
