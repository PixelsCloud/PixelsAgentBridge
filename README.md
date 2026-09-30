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
- **Desktop capabilities:** enumerate windows, return bounded JPEG screenshot previews
  to the AI agent or save original PNG files, and send supported mouse, keyboard,
  or Windows secure attention events.
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

The current source exposes **44 tools**. Hosts may display them with a namespace,
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
| `pab_capture_screenshot` | Return a bounded JPEG/PNG preview as MCP image content, or save an original screenshot with metadata and hash |
| `pab_desktop_input` | Send supported mouse, keyboard, or secure attention events |
| `pab_open_terminal` | Open an interactive terminal |
| `pab_terminal_input` | Send terminal input |
| `pab_terminal_read` | Read terminal output |
| `pab_terminal_resize` | Resize a terminal |
| `pab_terminal_close` | Close a terminal session |
| `pab_upload_file` | Queue a binary upload; optionally replace an existing regular file after verification |
| `pab_download_file` | Queue a binary download; optionally replace an existing regular file after verification |
| `pab_get_operation` | Read persisted transfer, command, file-operation or system-query results using its original request ID |
| `pab_list_operations` | List this MCP session's transfers, commands, file operations and system queries with device/state filters and a cursor |
| `pab_cancel_operation` | Request cancellation; query until the outcome is confirmed |
| `pab_disconnect` | Disconnect this MCP session from a device independently of Desktop and other MCPs |
| `pab_file_stat` | Inspect target file/directory metadata and links |
| `pab_file_read` | Read bounded text by line or byte range with a SHA-256 version |
| `pab_file_write` | Stage and publish text with explicit overwrite and optional version checks |
| `pab_file_patch` | Apply exact replacements against a checked original version |
| `pab_file_search` | Search literal names or text with glob filters and explicit bounded results |
| `pab_file_hash` | Start streaming SHA-256 in the background; query byte progress or cancel |
| `pab_mkdir` | Create directories with explicit parent/existing behavior and partial results |
| `pab_file_copy` | Copy files or explicit recursive trees within the target device |
| `pab_file_move` | Copy and verify the destination before removing source entries, including across filesystems |
| `pab_file_delete` | Remove a bounded, explicitly recursive manifest and report actual deletions |
| `pab_archive_create` | Create and verify a staged ZIP with explicit sources and overwrite |
| `pab_archive_extract` | Extract ZIP with path/type/conflict/size preflight and per-file integrity checks |
| `pab_system_info` | Query OS/host, CPU, RAM/swap and Executor identity; optionally query NVIDIA GPU data |
| `pab_list_disks` | Query mounts, filesystem, capacity, free space and disk flags |
| `pab_list_processes` | Collect a bounded process list with PID/name/user filters; no live pagination |
| `pab_get_process` | Query one current PID with available identity and resource fields |
| `pab_list_network_interfaces` | Query addresses, MAC, MTU, state and cumulative byte counters |
| `pab_list_network_connections` | Query TCP/UDP sockets, listeners and visible PIDs with address/port/state filters |
| `pab_resolve_dns` | Query DNS records through the target machine's configured DNS servers |
| `pab_list_sessions` | Query OS login sessions via Windows WTS or Linux logind |
| `pab_terminate_process` | Verify native process identity, request exit and optionally force termination |
| `pab_list_services` | Filter Windows SCM / Linux systemd service inventory by name and state |
| `pab_get_service` | Query one service's state, startup mode and available runtime information |
| `pab_service_control` | Asynchronously start, stop, restart, enable or disable a service |

Call `pab_connect` first and retain its platform context. Most device tools require
`device_code`; terminal follow-up tools use the returned `session_id`. Passwords
come from the local Bridge database and are not tool arguments.

Process termination and service management require system-query capability v3.
Get `termination_identity` from `pab_get_process`, then pass it as `identity` to
`pab_terminate_process`; seconds-resolution start time is not a substitute.
Windows verifies native creation time and retains the same process handle during
control. Linux retains the original pidfd with a bounded identity lease (10
minutes, at most 256 leases); expired leases or Executor restart require another
process query. There is no PID-only signal fallback.

Termination defaults to `force=false` and a 5000 ms graceful timeout. Windows
posts WM_CLOSE to top-level windows; windowless processes explicitly report
unsupported graceful exit, and require `force=true` for direct termination.
Linux sends SIGTERM through the retained pidfd and only escalates to SIGKILL on
timeout with `force=true`. Force-exit confirmation allows up to 5 additional
seconds. Only one process is controlled, not its tree; Executor and init/system
processes are protected.

Service queries use Windows SCM or the Linux systemd system bus (not user
systemd); Linux requires an exact `.service` name. Inventory uses literal
case-insensitive name filtering and exact backend-specific states, without live
pagination. Use `pab_get_service` for configuration details. Windows inventory
excludes kernel drivers; Linux includes installed `not_loaded` units.
Enable/disable changes startup configuration only: Windows enable selects
automatic startup, Linux changes persistent unit links without forcing masked
units. Runtime actions wait for observed state; Linux also checks the original
JobRemoved signal. Windows does not explicitly stop dependent services; systemd
may execute dependencies defined by the unit transaction. Executor's own service
cannot be stopped or restarted.

The two control tools return `running` after about 250 ms if still active. Keep
`request_id` and poll `pab_get_operation`. The same ID is never replayed;
disconnection/caller cancellation does not undo accepted actions, and service
timeouts never roll them back. Final results preserve phase, whether a change was
submitted, Linux job path, observed state and errors. Unconfirmed does not mean
unexecuted. Running operations currently expose overall state; detailed phases
are part of the final result. Concurrent controls of the same resource return
`resource_busy`. No new approval flow is added; OS permissions still apply.
Service timeout defaults to 30000 ms (100–60000 ms); blocking native SCM calls
cannot be forcibly interrupted. Dedicated Windows window/process and temporary
SCM service fixtures plus isolated QUIC acceptance passed. Linux backends pass
cross-compilation; Linux runtime and installed-host acceptance remain pending.

System queries use `sysinfo` and require target system-query capability version 1.
They return one sampled result with collection timestamps, not continuous monitoring
or an atomic OS snapshot. Process and interface lists default to 100 entries, accept
up to 1000, and are also bounded by a 32 KiB serialized result budget. Use filters
when `truncated` is true; there is no live pagination. Unavailable optional fields
are null. Process arguments and environment variables are not collected.

`sample_cpu` defaults to true for `pab_system_info` and false for process queries.
CPU sampling uses two observations at the library minimum interval;
`cpu_sample_ms` records the interval. `cpu_usage_basis_points` uses 10000 for 100%,
and process CPU may exceed 100% across cores. Capacities are bytes; CPU frequency
is MHz from the first logical CPU, not an all-core average. Network counters are
cumulative library counters, not instantaneous throughput.

`include_gpu=true` enables optional NVIDIA queries through dynamically loaded
`nvml-wrapper`. The GPU subsection reports its own status and field errors;
unavailable NVML does not establish the absence of GPUs. AMD/Intel GPU backends
are not implemented. System queries wait for completion and cannot be cancelled.
Blocking OS/driver reads have no hard interruption deadline; worker admission is
bounded, and a busy collector returns `executor_busy`. Each MCP permits 16 unresolved
system-query records. Reusing `request_id` reads the original sample through
`pab_get_operation`; omit it for a fresh sample. Interrupted or uncertain queries
are not automatically rerun. Desktop history shows query type and returned count.

These tools passed Windows local tests, including native process lifecycle,
result persistence and isolated QUIC. Installed-host, Windows/Linux two-machine
and full NVIDIA hardware acceptance remain pending. Existing installers do not
contain this batch.

Network connection, DNS and OS session queries require system-query capability
version 2; C1 queries remain usable with version 1. They share the same persisted
sample, output budget, task history and original-ID lookup rules.

Socket queries use `netstat2` with TCP/UDP, IPv4/IPv6, exact IP/port/PID and TCP
state filters. Lists have no live pagination. UDP peer/state are unavailable;
TCP listeners have null peers. Empty PID lists mean unobserved ownership. The scan
stops at 100000 entries or a soft 5-second elapsed budget, with explicit truncation;
blocking native inventory calls cannot be interrupted by that budget.

DNS uses `hickory-resolver` with freshly read target system DNS configuration,
without a public resolver fallback or local resolver cache. The default record type
is A; AAAA/CNAME/MX/NS/PTR/SOA/SRV/TXT are also supported. PTR accepts an IP or
reverse domain. Supply ASCII/IDNA names. Results contain owner/type/TTL and DNS
presentation text; long values set `value_truncated`. The lookup timeout defaults
to 5000 ms (100–10000); configuration loading is separate native I/O. This queries
DNS rather than the native OS resolver: hosts, mDNS and Windows NRPT/VPN split-DNS
policies are not consulted. Upstream servers may still cache responses.

Session queries list OS sessions rather than accounts, MCP sessions or PAB terminals.
Windows WTS can include service/listener sessions without a logged-in user. Linux
uses logind on the system bus with a 5-second collection deadline; missing logind,
access errors and unsupported platforms fail explicitly. Optional field failures
are returned in each entry's `errors`. User and state filters match exactly.
These three tools passed Windows local tests, deterministic local DNS fixtures and
isolated QUIC; Linux collector cross-compilation passed. Linux/macOS runtime and
installed-host acceptance remain pending.

Text tools require an updated target Executor. They support UTF-8 and UTF-16,
detect BOMs, and reject binary data instead of replacing undecodable bytes.
Reads return at most 16 KiB of UTF-8 text from files up to 4 MiB; writes/patch
inputs are limited to 128 KiB. Continue with `next_offset` and the returned
`metadata.sha256` as `expected_hash`. Patch edits use `find`, `replace`, and
`expected_matches` (default 1), match the original text, and cannot overlap.
Write/patch return `operation_ref`; reuse `request_id` for deduplication and use
`pab_get_operation` to resolve uncertain results. These bounded text operations
wait for completion and do not support cancellation. File bodies use the binary
device channel and are not persisted in operation history. Version checks and
PAB path locks do not provide atomic compare-and-swap against external editors.

Search/hash/mkdir require a target Executor with filesystem capability version 2;
existing text tools remain compatible with version 1. Search uses a literal
substring (not a regular expression), defaults to case-sensitive name matching,
and accepts `/`-separated relative globs such as `**/*.rs`. Hidden files are
included; gitignore is not applied. It returns at most 100 matches with 160-character
line previews and file hashes for content matches. Scans stop at 4096 entries,
64 MiB of accounted read budget, 5 seconds, or the output budget, reporting
`truncated`, `stop_reason`, skip counts and bounded warnings. Results are not an
atomic directory snapshot and have no live paging cursor.

`pab_file_hash` returns an `operation_ref` after remote acceptance and streams
256 KiB chunks without loading the whole file. Query `pab_get_operation` for
`progress.completed_bytes`, `progress.total_bytes` and the final `metadata.sha256`;
`pab_cancel_operation` requests a stop. Cancellation is only confirmed by
`cancelled`. The Executor permits four concurrent hash jobs, with a 30-minute
job limit and 30-second read timeout. Repeating `request_id` returns the original
operation, even if the file has since changed. Observed size/mtime/identity
changes fail the hash; this is not an atomic snapshot against external writers.
The MCP periodically refreshes active records; an offline query can return the
last saved state, so use the progress timestamp to assess freshness.

`pab_mkdir` defaults to `parents=false` and `exist_ok=false`. Its `created_paths`
lists directories actually created, including partial failure; it does not roll
back partial work. Uncertain results are queried using the original ID and are
never automatically replayed. Each MCP permits 32 unresolved filesystem records;
resolve existing operations before exceeding that limit. These tools do not
intentionally traverse symlinks or Windows reparse points. Path rechecks and PAB
locks do not eliminate races with external programs.

Copy/move/delete/ZIP require target filesystem capability version 3. They return
`operation_ref` after remote acceptance and run asynchronously; query or cancel
with the existing operation tools. Paths are exact source/target paths, with no
implicit basename append. `recursive=false` and `overwrite=false` are the defaults.
Directory copy/move can explicitly merge into an existing destination while
preserving unrelated entries. Move always copies and verifies before removing
source, even on one volume; it needs extra I/O and temporary disk space.

Bulk jobs use 64 KiB streaming buffers, four Executor workers and a cooperative
30-minute deadline. Defaults are 4096 entries, 1 GiB of file data and depth 64;
`max_bytes` can be raised to 8 GiB. ZIP extraction also counts implied directories
and its destination root toward the entry limit. ZIP input/staging is bounded by
`max_bytes + 2 MiB`; central-directory metadata is capped at 2 MiB. Extraction
accepts unencrypted stored/deflated entries with strict UTF-8 portable names,
rejects links, traversal, case collisions and file/directory conflicts, and checks
CRC/size/hash before each file is published. `max_ratio` defaults to 200 (1–1000);
highly compressible legitimate archives may be rejected by this limit.

`mutation` reports phase, processed/total entries, published/deleted counts,
`partial`, `source_removed` and bounded item results (64 items or 8 KiB).
Cancellation stops at checked boundaries; blocking OS I/O may delay it. Only
`cancelled` confirms the worker stopped. Completed effects remain after cancellation
or failure, including files already extracted before a later CRC error; nothing
is silently rolled back. Delete removes only planned entries and refuses roots.
Uncertain mutations are queried using their original IDs, never replayed. PAB
path locks now cover ancestors and descendants but do not eliminate external
writer races or make a tree operation atomic. ZIP does not preserve ACLs,
ownership or extended metadata.

These new file tools have Windows local automated coverage, including isolated
QUIC and MCP stdio tests. Installed-host, physical cross-volume and Windows/Linux
two-machine acceptance are still pending; older installers do not include them.

### Screenshot previews and original images

Screenshots use [xcap](https://github.com/nashaofu/xcap) for native capture and
[image](https://github.com/image-rs/image) for resizing, JPEG/PNG encoding and
bounded decoding. The product does not implement its own capture engine.

`pab_capture_screenshot` defaults to `mode="preview"`: JPEG, at most 1600×1000
pixels and 512 KiB of encoded image data. Quality starts at 75, then adapts down
to 45 and reduces dimensions if necessary. The response contains an MCP image
content block, a saved file and metadata; base64 is confined to the image block,
not repeated in text, structured results, SQLite or activity reports.

```json
{"device_code":"214601537"}
```

`mode="original"` defaults to PNG, preserves captured dimensions and uses an
8 MiB byte budget. Original JPEG is available explicitly, with quality 85 by
default. `format`, `quality` (JPEG only), `max_bytes`, preview-only `max_width`
and `max_height`, `monitor_id`, and monitor-relative `region` are supported.
Out-of-monitor regions fail instead of being clipped. Captures are limited to
16 Mi pixels and 16384 pixels per dimension. Each call captures a new frame;
same-frame region retrieval and window screenshots are not implemented yet.

```json
{"device_code":"214601537","mode":"original","destination":"C:\\Temp\\screen.png","include_image":false}
```

`destination` is optional. Its path must be absolute and its extension must
match the format. Existing files are never overwritten. Images above 512 KiB
are saved with an explicit `image_omitted_reason`, rather than returned as large
inline MCP payloads. `include_image=false` requests a file-only result.
Metadata includes the capture time, monitor, global crop origin, source and
encoded dimensions, actual JPEG quality, encoded bytes and SHA-256. Transport
uses existing binary frames, with codec, size, dimensions and hash validation.

These options require an upgraded Executor and desktop helper; unsupported
peers return an upgrade error, with no silent fallback to a different capture.
The legacy primary-monitor PNG request remains available for older clients.
Windows compilation, synthetic codec fixtures, isolated helper/QUIC tests and
MCP response tests are covered. Installed-host image display, native 4K/multiple
monitors and macOS/Linux graphical acceptance remain pending. Wayland capture
is explicitly unsupported by this product interface in this stage.

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

### Example: asynchronous transfer

Upload/download return an `operation_ref` immediately, even while the target is
connecting. `wait_ms` optionally waits up to 5000 ms; it does not limit the transfer.
Paths must be absolute for their respective machines (maximum 4096 characters).

```json
{
  "device_code": "123456789",
  "source": "C:\\work\\artifact.zip",
  "destination": "C:\\incoming\\artifact.zip",
  "overwrite": true,
  "request_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4"
}
```

Call `pab_get_operation` (or `pab_cancel_operation`) with the returned reference:

```json
{ "device_code": "123456789", "operation_id": "c746c0d6-f349-4e4d-92fa-9e3fb25abcf4" }
```

Reuse `request_id` with identical parameters to deduplicate submission, including
after completion or restart. Omitting it generates a new ID. `cancel_requested`
records intent, while `cancelled` confirms stopping; an already published file
still completes. `unconfirmed` means the original result is being checked, not
that it failed. Never replay it under a new ID. At most eight unresolved transfers
are accepted per MCP session. Active transfers must finish or resolve before
disconnecting. A disconnected device needs an explicit `pab_connect` to reconnect.
Historical records from an exited session can be queried but cannot be cancelled
by another session. Cached OS context is marked `remembered_device`.

Rebuild/install and restart the AI client to load the new tools. Older installed
MCP binaries retain their previous tool set and behavior. Update the target
Executor as well before using the text tools.

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
