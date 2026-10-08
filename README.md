# Pixels Agent Bridge

**Control your devices remotely with your AI agent.**

English · [简体中文](README.zh-CN.md)

Pixels Agent Bridge connects your local AI agent to remote computers through the
Model Context Protocol (MCP). An agent can select a device, inspect its operating
system, run native commands, transfer files, use an interactive terminal, and work
with supported desktop capabilities. The desktop app provides device management,
task history, and a live view of the MCP processes operating your devices.

The project is in active development. Windows, macOS and Linux command and file workflows
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

- **Server Web console:** React + Ant Design management for accounts,
  device lists and live online status, user bandwidth limits, Relay policy health and management
  changes. English, Simplified/Traditional Chinese and light/dark themes are included.
  Remote task records stay local. See the [deployment guide](WEB_DEPLOYMENT.md)
  and [development/test plan](WEB_DEVELOPMENT.md).

- **Agent-driven operations:** select devices and perform remote operations through
  a consistent set of `pab_*` MCP tools, with integrations for Codex, Kimi Code,
  Claude Code, DeepSeek Harness, and OpenCode.
- **Native commands:** execute a program with an explicit argument array, query its
  task state, and read stdout or stderr. Results include the verified target OS.
- **File transfer:** upload and download binary files using absolute paths, with
  integrity verification and explicit overwrite behavior.
- **Interactive terminals:** open, send input, read output, resize, and close remote
  terminal sessions.
- **Desktop capabilities:** enumerate windows, return JPEG screenshots at captured resolution
  to the AI agent, capture referenced windows, and send supported mouse, keyboard,
  or Windows secure attention events.
- **Device management:** remember and rename devices, copy their information, and
  distinguish online state from connection state. Connected cards show P2P or Relay.
- **Task history:** filter records by device, browse paginated results, and view a
  device's history alongside its information and remote tools.
- **MCP activity:** view live processes, client identities, device connections, tool
  calls, tasks, and transfer summaries on the MCP connections sidebar page.
- **Self-hosting:** deploy the control backend, PostgreSQL, and Relay using Docker Compose.

The desktop supports light and dark themes, Simplified Chinese, Traditional Chinese,
and English. It selects the system language on first launch and remembers your choice.

## Execution account and platform scope

Commands, PTYs, Git, file operations and transfers default to the device service
account. Use `pab_list_execution_contexts` to choose an explicit `user` context;
re-query after reconnect or login changes. Windows and macOS application tools
require a `desktop_user` context. Unavailable identities never fall back to the
service account. Results and local history retain the actual execution identity.

Linux is currently a headless Executor/MCP product, with no Linux desktop app.
Core tools work without a display or GUI login; desktop requests return an explicit
unsupported result. Command output defaults to UTF-8; `pab_read_output` and the
`pab_run_command` preview accept `encoding` (`utf8`, `gbk`, `gb18030`, `big5`,
`utf16_le`, `utf16_be`). History provides the same display selection. Changing it
re-reads stored bytes, never reruns the command. Preserve `next_offset` with the
same encoding: incomplete trailing characters remain for the next read; invalid
bytes at EOF report replacements. `include_base64` returns unfiltered raw bytes
for `[offset,next_offset)`. Arbitrary/tail starting character boundaries are not
guaranteed. File text tools have their own encoding selection. Keep original
request IDs when an outcome is unconfirmed.

Installed workflow evidence and hardware exclusions are in the
[execution acceptance report](acceptance/execution-e8-handoff-2026-10-07.md).

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

Installation requires a WSS control URL and an HTTPS Relay URL.
Use the same control service on computers that should communicate. For your own backend,
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

### 3. Enable an AI agent

Install your chosen client for the current OS user. In **Settings → AI Agent**, enable
Codex, Kimi Code (2.x or newer), Claude Code, DeepSeek Harness, or OpenCode independently.
Desktop verifies the bundled MCP executable and merges Pixels configuration and
tool grants without approving unrelated tools. Conflicting rules or malformed files
produce an error without overwriting the configuration. Start a new session or restart
the client to apply changes. “Configured” does not mean connected; disabling the
configuration does not terminate existing sessions.

OpenCode uses a local MCP command array and a `pixels_*` allow rule;
existing JSONC comments, other servers, and unrelated permissions are preserved.
Live agent sessions are shown on the **MCP connections** sidebar page below **Device list**.

For manual Windows setup, use the following Codex configuration, adjusting the path:

```toml
[mcp_servers.pixels]
command = "C:\\Program Files\\PixelsAgentBridge\\pab-mcp.exe"
default_tools_approval_mode = "approve"
```

`approve` pre-approves tool calls. `auto` may still request approval based on tool
annotations. Remote device authentication and operating system permissions still apply.

macOS Desktop provides the same integration controls. Headless Linux and other MCP
clients can register the installed `run-mcp.sh` entry point manually. See the
[integration plan and acceptance report](acceptance/agent-integrations-2026-10-08.md)
for configuration paths, tested client versions, and validation limits. These source
changes require updated Desktop and MCP binaries; existing installers do not include them.

Linux is a **headless** distribution containing Executor and MCP only. See
[Linux installation](packaging/desktop/unix/INSTALL-LINUX.txt) for systemd and
container installation, device credentials, upgrades and data retention. No Desktop
or graphical login is required. The headless packaging change has passed isolated
installation tests; the new release's installed-host remote acceptance is pending.

### 4. Ask the agent to operate a device

Example requests:

> Connect to device 123456789, confirm its operating system, and show free disk space
> on its system drive. Use Pixels tools and choose commands for the target OS.

> Upload C:\build\app.zip to C:\Users\Operator\Desktop\app.zip on device 123456789.
> Replace the existing file and verify its SHA-256 after uploading.

Replace the device code and paths with your own information.

## MCP tool reference

The current source exposes **68 tools**. Hosts may display them with a namespace,
for example `pixels.pab_connect`.

| Tool | Purpose |
|---|---|
| `pab_list_devices` | List locally remembered devices, including while the control server is offline |
| `pab_list_execution_contexts` | Discover native service/user identities and available verified desktop-user sessions |
| `pab_list_apps` | Discover installed or running applications in an explicitly selected Windows/macOS user session |
| `pab_launch_app` | Launch or activate an application by native ID or absolute application path |
| `pab_open_file` | Open a target-local file with a selected or default application |
| `pab_connect` | Authenticate a device and return verified OS and execution context |
| `pab_run_command` | Start a native executable with an explicit argument array |
| `pab_get_task` | Query state, progress, completion, and output ranges |
| `pab_read_output` | Read retained stdout or stderr by offset |
| `pab_list_directory` | List a page of directory entries |
| `pab_list_windows` | List a bounded window snapshot with opaque references, geometry, PID, monitor and observed state |
| `pab_list_monitors` | List display IDs, origins, dimensions, primary state, scaling and rotation |
| `pab_focus_window` | Focus a referenced window, respecting OS foreground policy |
| `pab_window_control` | Minimize, maximize, restore or request normal close of a referenced window |
| `pab_type_text` | Enter Unicode text into an explicitly referenced foreground window |
| `pab_ui_query` | Query a bounded accessibility subtree of a referenced window or control |
| `pab_ui_get` | Read a referenced control, optionally including its non-protected value |
| `pab_ui_action` | Invoke, set text/check state, select, expand, collapse or focus a supported control |
| `pab_ui_wait` | Wait asynchronously for a bounded control condition without holding the input queue |
| `pab_capture_screenshot` | Capture current desktop or referenced window as JPEG at captured resolution, with metadata and hash |
| `pab_desktop_input` | Send a legacy input event or an ordered window-bound batch of desktop actions |
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
| `pab_git_status` | Read repository status, branch/HEAD, upstream and conflicts |
| `pab_git_diff` | Read bounded worktree, staged or commit diffs with literal file selections |
| `pab_git_log` | Page commit history from a pinned starting commit |
| `pab_git_commit` | Commit explicitly selected files while preserving unrelated staged changes |
| `pab_git_checkout` | Switch an existing local branch or detach at a selected commit |
| `pab_git_fetch` | Asynchronously fetch a configured remote |
| `pab_git_pull` | Asynchronously pull with an explicit ff-only, merge or rebase strategy |
| `pab_git_push` | Asynchronously push a selected branch, with an explicit lease for force mode |
| `pab_list_containers` | List target Docker containers with name, state and label filters |
| `pab_get_container` | Inspect a pinned container's state, image, ports, mounts, networks and health |
| `pab_container_logs` | Read finite, bounded Docker logs with stream and time selection |
| `pab_container_control` | Asynchronously start, stop or restart and confirm the observed state |

Monitor-targeted input adds `monitor_input` (Executor system v8 / helper v3). Copy `input_target` from `pab_list_monitors`, use monitor-relative logical coordinates, and verify the application effect. Stale display geometry or helper instances are rejected. See [the coordinate contract and repeatable acceptance suites](acceptance/README.md).


Call `pab_connect` first and retain its platform context. Most device tools require
`device_code`; terminal follow-up tools use the returned `session_id`. Passwords
come from the local Bridge database and are not tool arguments.

Terminal open results also report `startup.arguments` and `startup.mode` from the
actual launch: Windows uses `powershell.exe -NoLogo -NoProfile`
(`interactive_no_profile`), macOS uses `/bin/zsh -l -i` (`interactive_login`), and
Linux uses `/bin/sh -i` (`interactive`). macOS therefore loads login/interactive
shell configuration under the selected account; Linux follows `/bin/sh`'s
interactive configuration rules. Noninteractive command tools do not load shell
startup files themselves. An older peer that omits `startup` is reported as unknown,
not inferred from its OS. Desktop shows the returned shell and arguments.

### Tool group settings

Open **Settings → MCP tools** to choose six static groups. All 68 tools are enabled by default, preserving existing use. Connections/tasks (15) are required; files (15), system (12), desktop (14), Git (8), and Docker (4) can be disabled individually. Save, then restart the MCP process through your AI client to load the selection. Restarting only Desktop does not reload an already running MCP process.

Preferences are stored per user in `mcp-tools.json` under the PAB data directory (`PAB_DATA_DIR` when set). Each MCP reads a fixed startup snapshot; existing processes and accepted operations retain their behavior. Required operation query/cancel tools remain available when optional groups are disabled. Tool listing and dispatch use the same selection, so explicitly calling an omitted tool fails before runtime initialization. Groups control discovery and dispatch, not device permissions or approval policy; calls retain the configured automatic execution behavior.

Configuration has version 1 and a required `enabledGroups` array of `core`, `file`, `system`, `desktop`, `git`, `container`; `core` must be present. Unknown fields/groups, duplicate groups, unsupported versions and malformed files fail MCP startup with an error rather than silently enabling everything. The settings editor displays the error and allows an explicit replacement by saving a valid selection. Saves use tempfile atomic replacement, so startup never sees partially written JSON.

### Execution users and application tools

These additions are implemented and included in the first acceptance packages;
remaining fixes and installed-host acceptance are tracked in [the execution roadmap](EXECUTION_CONTEXT_ROADMAP.md).
An older MCP/Executor installation does not gain them by updating these docs.

Call `pab_list_execution_contexts` on the same device connection and use the returned
`selection` object unchanged. `service` is the existing default for commands/files;
`user` selects a native account for commands, terminals, Git, files and transfers;
`desktop_user` selects a verified Windows/macOS interactive helper for applications.
User command/terminal/Git/filesystem/directory/transfer options require capabilities
v3/v2/v11/v5/v6/v2 respectively. Explicit user execution never falls back to the service.
References are bound to the device, caller and connection; rediscover after reconnect
or login changes. Linux headless supports native user execution without a desktop or
logind, but application tools explicitly return unsupported.

Git uses the selected account's configuration and credential helpers. Selecting a
user does not unlock a keychain, make an unavailable SSH agent accessible, or grant
interactive authentication. Configure credentials for that user before background
work; inspect the original operation if authentication fails or times out. Tests
with isolated SSH agents passed on Windows, macOS and Linux. The isolated macOS
Keychain test covers unlocked/locked/restored access through a user worker; it does
not promise automatic access to every login keychain or third-party helper.

Application tools require system-query **v12** and an available user helper. Use
`pab_list_apps` with `scope="installed"` (default) or `"running"`, optional literal
`search`, and `limit=1..200` (default 100). This is a bounded snapshot, not live paging.
Pass either `{"kind":"id","id":"<returned OS ID>"}` or
`{"kind":"path","path":"<absolute executable or .app path>"}` as `application`
to `pab_launch_app`. `pab_open_file` takes an existing absolute target-local `path`;
omit `application` to use that user's default. No URL schemes or arbitrary arguments.
All three require the `desktop_user` selection in `execution`.

On macOS, `pab_launch_app` additionally accepts `new_instance: true` when you explicitly
need a separate instance rather than the default OS reuse behavior. This requires
system-query v13 and application helper v2; older helpers reject it before dispatch.
Windows rejects this option. The application may enforce its own single-instance policy,
and neither mode guarantees a ready window. Inspect the returned process and window list.

Setting a text control changes its value; it does not verify that the document was saved.
For Save As, navigate to the directory separately from entering the filename, then read
the resulting file to verify its content. Do not replay a shortcut just because opening
a new window changed focus and the batch result became unconfirmed.

Known macOS TextEdit issue, reproduced on installed 1.2.42: closing an unsaved document
after an AX edit may omit the save prompt or save an empty file from the close dialog.
Explicit Save, file readback, then Close has been verified; the unsaved-close issue remains
unresolved. See the [compatibility acceptance record](acceptance/compatibility-release-2026-10-08.md).
The subsequent [session retest](acceptance/compatibility-session-resume-2026-10-08.md)
successfully saved both rich-text and plain-text documents from the close dialog.
The earlier failure was not reproduced in that retest; its root cause remains unconfirmed.

The result reports native execution identity and an observed instance when available.
OS acceptance does not prove a new process, a ready window or changed document content.
Use the existing window/UI tools to observe, focus, interact and request normal close;
an unsaved-document prompt is not bypassed. A missing, locked or changed desktop does
not select another account. Preserve `request_id` and query `pab_get_operation` on
running/unconfirmed results; never automatically repeat a launch after a lost reply.
These actions cannot be cancelled or rolled back, and native OS calls have no guaranteed
hard interruption. Operation data stays in the local SQLite history.

In Desktop's device **Remote tools** panel, commands, directory browsing, terminals
and file transfers share an **Execute as** selector. Refresh discovers available
users; an expired explicit selection disables submission until selected again.
**Applications** has a separate, explicit desktop-session selector. Task details
show the observed account/session, or **Not recorded** for older records.
Transfers use the same durable queue as MCP. Cancellation is a request; an unknown
publication remains **Unconfirmed**, with **Inspect original operation** to query
its result without resending the file. These UI additions have source/browser
coverage. Installed Windows 1.2.32 passed three-language/light-dark checks of settings,
actual terminal-history identity, persistence when choosing the current language,
and task-row/status layout. This is not full workflow or macOS UI acceptance.
See [the installed UI report](acceptance/execution-e7-ui-refresh-2026-10-07.md).

### Docker container operations

The four Docker tools use [Bollard 0.21.1](https://docs.rs/bollard/0.21.1/bollard/)
to access the [Docker Engine API](https://docs.docker.com/reference/api/engine/).
They require target system-query capability **v6** and a running Engine accessible
to the Executor's OS identity; the Docker CLI is not required. The connection uses
local `DOCKER_HOST` (Unix socket or Windows named pipe) or the standard local
endpoint. Docker CLI contexts, a logged-in user's Docker Desktop environment,
and remote TCP/SSH endpoints are not implicitly reused. Rootless or nonstandard
sockets must be configured for Executor. API version negotiation is applied to
actual request paths through Bollard's request modifier.

`pab_list_containers` defaults to running containers (`all=false`). Use `all=true`
for stopped containers; `name` is a case-insensitive literal substring, `states`
contains exact native states, and `labels` uses `key` or `key=value` filters.
There is no live pagination or atomic snapshot. It scans up to 10000 entries and
returns at most `limit` (default 100, maximum 1000), additionally subject to the
32 KiB serialized result budget. Truncation is explicit.

Get/log/control select an **exact name or full 64-character ID**. Abbreviated IDs
are rejected; after resolution, all follow-up actions use the original full ID.
Results also identify the Engine OS, architecture and version, separately from the
Executor OS. Inspect returns selected state/health, ports, mounts, networks, restart policy,
image and log-driver fields; environment, command arguments and raw inspect data
are excluded. Labels and logs are application-provided data and can still contain
sensitive values. Detail collections and individual strings are bounded.

Logs default to `tail=200`, timestamps and both streams enabled, `follow=false`.
`since`/`until` use Unix seconds up to 2147483647 (`until` must be positive);
`tail` is Docker's native selection, at most 1000, without a per-stream line-count
guarantee. Raw retained text is limited by `max_bytes` (default/maximum 16384,
minimum 1024), then by the serialized result budget. Text is grouped by stream,
without a cross-stream ordering guarantee. TTY output is merged; stderr-only
selection fails. Split UTF-8 is assembled within each stream; invalid encoding
has an explicit replacement warning. This is a finite log tail, not a lossless
resume cursor. Unsupported log drivers and Engine errors are returned directly.

Control accepts `start`, `stop` or `restart`. It returns running after about
250 ms, with the original request ID usable through `pab_get_operation`.
Start/stop already at the desired state sends no action. Restart requires both
running state and a different `StartedAt`. Stop/restart use Docker's normal
stop timeout (`stop_timeout_seconds=10`, range 0–120); Docker may kill the
container after that timeout. A successful HTTP acknowledgement alone does not
mark the operation completed. No container creation/removal or image pull is
part of these tools.

Results distinguish `submission_started` (the submission phase was entered;
it may have sent an action), `daemon_acknowledged`, and
`desired_state_observed`. Cancellation stops waiting, not Docker's accepted
action. Timeout, connection loss or restart can leave an `unconfirmed` result;
the same request ID never replays it. Other PAB controls on that engine/container
stay busy while a submitted result is unresolved, including after Executor
restart. Query the original request to inspect the pinned engine/container
without resubmission. Matching state confirms the desired outcome, not which
caller caused it; external Docker clients can still change state concurrently.

Local HTTP-fixture tests, QUIC, stdio and a real Windows named-pipe workflow on
Docker Desktop's Linux Engine pass. The live test creates and removes its own
isolated container and verifies Chinese stdout, stderr, start/restart/stop and
inspection. macOS fixture/protocol tests also pass; a real macOS Docker Engine,
installed-host Pixels calls, Linux runtime and Windows containers remain unverified.
Upgrade both MCP and Executor and restart the AI client. For the macOS package,
see [MACOS.md](MACOS.md).

### Git operations

Git tools require target system-query capability **v5** and native Git (2.23 or
newer for `switch`). They reuse [Git](https://git-scm.com/docs)'s repository,
transport, credentials and hooks through explicit program arguments without a
shell. Without `execution`, configuration belongs to the Executor's service identity;
a Windows SYSTEM service does not automatically use the logged-in user's keys or
Git author configuration. To use that account, discover and pass its `user` selection
(system-query **v11** or later). Missing Git, authentication and permissions errors are
returned directly. Remote parameters name existing configured remotes, such as
`origin`; URLs and passwords are not tool arguments.

`repo` is an absolute path on the **target** computer. Status is a current,
bounded observation, not an atomic repository snapshot. Diff disables external
programs/text conversion and summarizes binary changes. For history pagination,
retain `start_commit`, use it as the next request's `start`, and use the returned
`next_skip`; this also handles pages shortened by the byte budget. Each result
fits 32 KiB, with explicit truncation; requests must fit 60 KiB.

Example tool arguments:

```json
{
  "device_code": "123456789",
  "repo": "C:\\work\\project",
  "limit": 20,
  "request_id": "8b7f1714-32dc-4b79-bd87-96a1d2101af0"
}
```

Use these with `pab_git_log`. Commit requires individual literal relative paths
(or tracked deletions) and an explicit message; directories are rejected. It
stages only those paths and commits their current worktree contents, leaving
unrelated staged changes intact. A failed commit may leave selected files staged.
Checkout does not force overwrites or stash. Pull requires a clean, attached
working tree and an explicit `strategy`: `ff_only`, `merge`, or `rebase`.
Conflicts remain available for inspection; they are not automatically aborted.

Mutations still running after about 250 ms return an operation reference. Keep
`request_id` and query `pab_get_operation`; repeating the same ID never reruns the
operation. Network deadlines default to 300000 ms, others to 30000 ms.
`pab_cancel_operation` requests stopping the owned Git process, not rollback;
service-mode SSH or hook descendants may outlive it. Explicit user operations
reap their owned worker tree before releasing the repository lock; this cannot
roll back effects or stop work submitted to an independent service. Once a mutation starts, timeout or
cancellation can leave its result `unconfirmed`. Other PAB operations on the same
discovered Git directory return busy while it runs; Git's own locks still apply.

Push pins the selected commit ID. `force=false` is the default; `force=true`
uses an explicit lease against the observed remote reference. If a push receipt
is lost, a later query can check that exact commit/reference without pushing
again. A matching reference confirms the desired remote state, not which process
published it. No automatic reset, force-push, identity creation or hook bypass is
performed. Request identity hashes the commit message; Git results may still
contain commit messages and are retained in task history.

Windows and macOS temporary-repository, local bare-remote, cancellation,
persistence, QUIC and stdio tests pass. User-worker SSH authentication tests now pass
on all three platforms. An installed Linux user also completed all eight Git tools
against an isolated local remote; this does not prove installed-host SSH authentication
or the full two-round workflow. See [the Linux report](acceptance/execution-e8-linux-first-2026-10-07.md).
Rebuild both MCP and Executor and restart the
AI client; macOS package/verification details are in [MACOS.md](MACOS.md).

### System queries

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
are not implemented. System reads return their current state after waiting up to five seconds; query
the original ID if still running. Blocking OS/driver reads cannot be interrupted.
The Executor runs at most two system queries and admits sixteen running/queued
requests, with a five-second wait for each execution slot or collector lock.
A full queue returns `executor_busy`; an expired queue wait returns `queue_timeout`,
without resampling. Queued Git/Docker requests can be cancelled before dispatch. Each MCP permits 16 unresolved
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
isolated QUIC; Linux collector cross-compilation passed. macOS native collection
and automated tests also pass; Linux runtime and installed-host acceptance remain
pending. macOS sessions use utmpx; see [MACOS.md](MACOS.md).

Text tools require an updated target Executor. They support UTF-8 and UTF-16,
detect BOMs, and reject binary data instead of replacing undecodable bytes.
Normal lines/bytes reads return at most 16 KiB of UTF-8 text from files up to 4 MiB; writes/patch
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
substring by default, defaults to case-sensitive name matching,
and accepts `/`-separated relative globs such as `**/*.rs`. Hidden files are
included; gitignore is not applied. It returns at most 100 matches with 160-character
line previews and file hashes for content matches. Scans stop at 4096 entries,
64 MiB of accounted read budget, 5 seconds, or the output budget, reporting
`truncated`, `stop_reason`, skip counts and bounded warnings. Results are not an
atomic directory snapshot; newer Executors support checked continuation as described below.

Filesystem capability v4 enhances the existing tools:

- `pab_file_read`: `mode: "tail"` reads the final `tail_bytes` (default 16384); `mode: "follow"` resumes a `cursor` or raw byte `offset`, optionally waiting `wait_ms` (0–5000). These modes use bounded reads for large logs without the normal 4 MiB file ceiling, do not calculate a full-file hash, and reject `expected_hash`. `max_bytes` is 4–16384 and returned UTF-8 text is also capped at 16 KiB. Resume using `result.log.cursor` and the same encoding in a new read request. Incomplete final UTF-8 characters and UTF-16 code units/surrogates remain for the next call and set `incomplete_character`; an empty wait sets `wait_expired`. File identity uses `same-file`; head/boundary byte anchors detect observed replacement, truncation and relevant rewrites as `log_changed`. This is not a full-file historical snapshot and cannot detect every external rewrite in the middle. Tail starts at an encoded-character boundary and may omit an incomplete initial character. BOM-less UTF-16 still requires explicit encoding.
- `pab_file_search`: `regex`, `exclude`, `context_lines`, and `cursor`. [Rust regex](https://docs.rs/regex/1.13.1/regex/struct.RegexBuilder.html) supplies regular expressions with bounded compilation; unsupported syntax fails explicitly. `globset` exclusions such as `["node_modules/**", ".git/**"]` prune directories. Context includes up to five lines on each side, with 160-character previews per line. Resume `result.search.next_cursor` with the same query and filters. Sorted directory metadata and the continuation file hash are checked; observed changes return `search_changed`. Incomplete inventory (depth, entry or time budget) supplies no continuation cursor: inspect `truncated`, `stop_reason`, and warnings. The five-second scan budget is checked between I/O/lines, not a hard interruption of a native call or regex match.
- `pab_file_patch`: `dry_run: true` reuses the existing nonoverlapping multi-edit and hash checks. `patch_preview` reports original/result hashes, result size, whether content changes, and per-edit match counts without publication. Apply with a **new request_id**, `dry_run: false`, and the original `expected_hash`; intervening changes are rejected. Preview neither reserves the file nor introduces an approval step.

For example, read the last 8 KiB and then wait for appended text:

```json
{ "device_code": "123456789", "path": "C:\\logs\\app.log", "mode": "tail", "tail_bytes": 8192 }
```

```json
{ "device_code": "123456789", "path": "C:\\logs\\app.log", "mode": "follow", "cursor": "previous result.log.cursor", "wait_ms": 5000 }
```

Enhanced options require updated MCP and filesystem-v4 Executor installations; older peers reject them before execution rather than ignoring them. Basic requests retain their legacy wire format and deduplication fingerprints. Windows/macOS local automation and isolated QUIC tests passed; installed-host and native Linux acceptance remain pending.

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

### Accessibility control tools

The four `pab_ui_*` tools require system capability v9 and desktop helper v4.
Windows uses pinned `uiautomation 0.25.1` (Apache-2.0) and Microsoft's Windows
bindings; macOS uses `accessibility`/`accessibility-sys 0.2.0` (MIT/Apache-2.0).
Native objects live in a recoverable internal `--ui-worker` process. This is
part of the desktop executable, with no additional network connection.

Start with `pab_list_windows`, then query that `window_ref`. For example:

```json
{"device_code":"123456789","scope":{"type":"window","window_ref":"<returned UUID>"},"selector":{"role":"text_field","name":"Search"}}
```

Pass those arguments to `pab_ui_query`. Choose an explicit returned `element_ref`
for `pab_ui_get` or `pab_ui_action`; names need not be unique. `set_value` uses
`"action":{"type":"set_value","value":"hello"}`. Values are omitted from queries
and action results; explicit get can request `"include_value":true`. Password
values are never read or set. Unnamed table rows can be identified by querying
their subtree and reading a child cell. Only advertised `supported_actions`
are available; there is no implicit coordinate-click or keyboard fallback.

Queries default to 100 results, depth 6 and 3 seconds; maximums are 500 results,
depth 12, 2000 visited nodes and 10 seconds. Responses fit 32 KiB and report
truncation. Narrow the scope instead of paging a changing tree. Wait accepts
exists/absent/enabled/value_equals/checked/selected, defaults to 5 seconds and
250 ms sampling, and permits up to 30 seconds. Partial queries cannot prove
absence, and non-unique value/state matches return ambiguous.

References belong to one authenticated connection and helper session. Closed
windows, restarted workers and expired references require a fresh query.
Use `expected` preconditions to reject changed controls; OS validation and
action dispatch cannot be atomic against other desktop users. Operations that
take over about 250 ms return an operation reference. Reuse `request_id`, poll
`pab_get_operation`, and never replay an unconfirmed action. Cancellation does
not undo dispatched effects. `verification` distinguishes native API return
from observed desired state; invoking a button does not certify business success.
Local summaries omit control names and entered text. Explicit query/get replies
are retained locally and may be retained by the AI host.

Implementation and current acceptance boundaries: [UI roadmap](UI_AUTOMATION_ROADMAP.md)
and [UI verification report](acceptance/ui-u0-2026-10-06.md). Linux accessibility,
secure desktops and arbitrary custom-drawn controls are outside this delivery.

### Referenced window control and Unicode input

Display/window enumeration uses [xcap](https://github.com/nashaofu/xcap); Unicode
input uses [Enigo](https://github.com/enigo-rs/enigo). Foreign-window state changes
use the Windows official bindings or [x11rb](https://github.com/psychon/x11rb)
with standard EWMH messages. These tools require Executor system capability v4
and an upgraded, active desktop helper. Windows and Linux/X11 adapters are
implemented; Linux graphical runtime acceptance is pending. Wayland is rejected
explicitly. macOS uses public Accessibility APIs for window control and Enigo
for input; Screen Recording/Accessibility permission is required. Native builds
and automated tests pass; actual graphical acceptance is pending permission.
See [macOS implementation, installation and verification](MACOS.md).

Call `pab_list_windows`, take the returned `window_ref`, then focus or control
that reference. Call `pab_focus_window` before `pab_type_text`. Window lists contain
at most 64 entries, monitor lists at most 32, and replies at most 32 KiB; truncation
is explicit. Enumeration follows xcap filters (on Windows it excludes the helper's
own process, hidden/cloaked windows); entries disappearing or becoming inaccessible
during sampling are skipped. Coordinates use the native xcap coordinate space, not screenshot
preview pixels. A reference belongs to one helper connection and is invalid after
reconnection, desktop switching, window destruction or removal of its identity
marker. The helper checks the PID and marker before acting. External applications
can still change windows or focus between checks; these checks are not an atomic
window lock.

Text accepts 1–4096 UTF-8 bytes, including Unicode, without NUL. It does not replace
the clipboard. API acceptance does not verify the application's resulting text;
focus changes or application behavior may cause partial input. Close sends a
normal close request and never terminates the process. A save dialog/refusal can
produce a failed result with an unconfirmed side effect.

Keep `request_id`/`operation_ref`: repeated IDs never repeat a mutation. A slow
mutation returns `running`; query `pab_get_operation` to retrieve the original
result after disconnecting. A dispatched operation whose helper disappears has
an unconfirmed outcome. Executor restart preserves accepted mutations as
unconfirmed and never replays them. These operations cannot be cancelled/undone.
The persistent identity stores a BLAKE3 digest of input text, not the raw text;
this is audit minimization, not encryption, and does not protect predictable text
from guessing. Text still reaches the target and may be retained by the AI host.

### Ordered desktop actions

`pab_desktop_input` accepts either its original `event` or a batch with `window_ref`,
`actions`, optional `request_id`, and `timeout_ms` (default 5000, range 100–10000).
Batch support requires Executor system capability v7 and desktop helper v2; older
helpers continue to support the original single-window tools. For example, after
listing windows, pass an actual returned reference:

```json
{
  "device_code": "123456789",
  "window_ref": "REPLACE_WITH_RETURNED_WINDOW_REF",
  "actions": [
    {"type": "focus"},
    {"type": "key_chord", "modifiers": ["control"], "key": "a"},
    {"type": "type_text", "text": "Hello 世界"}
  ]
}
```

A batch contains 1–32 steps: `focus`, `control`, `type_text`, `key_chord`, `click`,
`scroll`, or `wait`. All steps target the same window. Text totals at most 4096
UTF-8 bytes; waits are 1–2000 ms each and total less than the batch deadline.
Key chords use unique `control/alt/shift/meta` modifiers and a lowercase letter,
digit, `f1`–`f12`, or a named navigation key such as `enter`, `tab`, or `page_down`.
Clicks use `x/y` relative to the current **outer window** in xcap native coordinates,
not screenshot pixels. Clicks verify the pointer target; scrolling requires the
pointer already over the referenced foreground window (`axis=horizontal/vertical`,
nonzero `amount` between -100 and 100; positive means right/down).

The active helper handles the entire batch before the next helper request, including
requests from other agents. This does not exclude physical input or other programs.
Input steps check window identity and foreground; the helper checks its active
desktop between steps and during waits. On failure, later steps are skipped. Read
`result.data.snapshot.batch` for zero-based indices, completed count, failed step,
and each step's `completed/failed/unconfirmed/skipped` state. Completed effects are
not rolled back; API acceptance does not prove application content. Keys/buttons
are released on ordinary error paths, but process termination cannot guarantee cleanup.

The timeout stops dispatching further steps; it cannot interrupt a blocked native
call. A slow batch may return `running`: query its original `operation_ref`.
Reuse `request_id` to observe the same batch, never to replay it. Text and chord-key
values are hashed in persistent request identity. Helper loss or Executor restart
may leave the result unconfirmed; it is never automatically resubmitted.

### Current desktop and window screenshots

Screenshots use [xcap](https://github.com/nashaofu/xcap) and [image](https://github.com/image-rs/image).
`pab_capture_screenshot` encodes once as JPEG at the captured resolution, with
quality 85 by default (optional `quality`: 30–95). It does not resize, adapt quality,
or impose an image byte limit. `max_width`, `max_height`, `max_bytes`, PNG and raw-image
mode are absent from the MCP interface. Each call captures the current picture
and drops the source pixels after encoding; no raw-frame cache or `capture_id` is kept.

```json
{"device_code":"214601537"}
```

For a specific window, first obtain its `window_ref` from `pab_list_windows`:

```json
{"device_code":"214601537","window_ref":"<window_ref from pab_list_windows>"}
```

JPEG capture requires screenshot capability v3 and an upgraded active
helper connection. It does not focus/restore the window. Stale references,
minimized/unavailable windows or geometry/display changes observed during capture
fail; no alternative window is selected. It follows xcap's capture/visibility and
permission limitations; protected/GPU-rendered surfaces are not guaranteed.
`window_ref` cannot be combined with `monitor_id` or `region`. For desktop captures,
`region` remains relative to the selected monitor and out-of-bounds regions fail.

Metadata includes the window reference/client rectangle when applicable, capture
time, monitor, origin, source/encoded dimensions, actual quality, size and SHA-256.
`preview_to_desktop` maps `desktop = origin + preview_pixel * scale` into xcap native
coordinates; it uses the captured window coordinate extent, including when DPI
makes source image pixels differ. It describes capture time, so recheck the window
before input. These coordinates are not normalized 0..65535 mouse input values.
The former 16 Mi pixel and 16384-dimension budgets do not apply to this JPEG mode.
JPEG itself represents dimensions up to 65535 per axis. Geometry checks do not
atomically lock a window.

Compressed results remain in the existing screenshot history. `destination` is
optional, absolute, create-only, with a `.jpg` or `.jpeg` extension;
`include_image=false` returns only the compressed file and metadata. Base64 appears
only in the MCP image content block, never duplicated in JSON/SQLite/reports.
The helper and network send binary chunks, without a total image size cap, and
verify actual codec, dimensions, size and hash. Desktop capture and live preview
also use this JPEG mode.

JPEG mode requires v3 even for monitor captures; older helpers fail with an upgrade
message. Legacy internal preview/primary-monitor PNG requests remain compatible.
Windows owned-window capture/negative-origin-DPI mapping, large JPEG encoding,
and images above 8 MiB through isolated helper/QUIC/history/MCP response tests have
been exercised. Physical 4K/multi-screen,
installed-host display, Linux/macOS graphical acceptance remain pending.


### Enhanced commands, waits and output

- `pab_run_command` accepts `request_id`, `env`, `stdin_text`, `timeout_ms` and `wait_ms`. Reusing an ID with identical parameters returns the original task; conflicting parameters are rejected. Keep the original ID when retrying.
- `env` overrides the child's inherited environment (64 entries / 8 KiB total). `stdin_text` accepts up to 16 KiB of UTF-8 and closes stdin after writing. `timeout_ms` accepts 1–86400000; omitting it adds no execution deadline. Timeout stops the direct child, without guaranteeing descendant cleanup. Incomplete output draining is reported as a failure.
- Command `wait_ms` defaults to 0 and permits up to 30000. It applies after remote acceptance, not to connection or submission time. Completion within the wait includes the final 8 KiB of each output stream; use `pab_read_output` for earlier bytes. Wait expiry returns the original reference, not a failed task.
- `pab_get_task` / `pab_get_operation` accept `wait_ms`, `wait_until: "change" | "complete"`, and `after_revision`. They return `revision`, `changed`, and `wait_expired`. Carry the revision into the next query. The wait budget includes the initial snapshot read, but that first read is not forcibly cancelled.
- `pab_read_output` accepts `max_bytes` (4–65536), `tail_bytes` (1–65536, mutually exclusive with offset), `contains`, and `wait_ms`. Resume with `next_offset`. Filtering matches lines in the returned chunk only, without cross-chunk guarantees; the cursor advances over all scanned bytes. Retention gaps appear in `gap`; lossy UTF-8 decoding appears in `decoding_replacements`.
- `pab_connect` waits 5000 ms by default (configurable 0–30000). Pending calls return `state: "connecting"` and a `connection_ref`. Another call for the same device reuses the active attempt; `pab_disconnect` can stop it. A background attempt lasts approximately 120 seconds at most and reports failure before a new attempt can be started.
- Tool errors preserve the message and include `error.code`, `phase`, `retry_action`, and an available request reference. Query the original operation when the network outcome is uncertain; do not automatically retry with a new ID.

`env`, `stdin_text`, and `timeout_ms` require command schema v2 on the target Executor. Older peers are rejected before command submission instead of silently ignoring options. Plain legacy commands remain compatible. These source features require updated MCP and target Executor installations.

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

Default guest MCP processes reserve separate persistent keys under
`mcp-endpoints/guest-<slot>.key` in the user data directory. OS file locks keep
live processes in distinct slots and release the slots on exit or a crash;
retries keep the same identity, and later processes can reuse free slots.
Desktop retains `guest-endpoint.key`. Device history and saved passwords remain
in the shared database. Do not delete slot keys or lock files while MCP is running.
Manually configured account MCPs (`PAB_MCP_GUEST=0`) require a separately registered
`PAB_ENDPOINT_SECRET_FILE` per concurrent process; a busy key returns an error.
Existing MCP processes must be restarted after installing this fix.

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
Check **MCP connections** in the sidebar to inspect agent activity.

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
| Linux distribution | Headless Executor + MCP; no Linux desktop package. Legacy GUI components are outside current delivery scope |
| macOS | Native app/launchd package and backends implemented; ARM automated tests and Intel compilation pass; graphical and deployed-host acceptance remain limited by permissions/environment. See [MACOS.md](MACOS.md) |
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

Install frontend dependencies once, then build and package from the repository root:

```powershell
npm --prefix apps/desktop ci
python scripts/build.py desktop --package
```

To repackage the same build with different control/Relay URLs:

```powershell
python packaging/desktop/build_nsis.py --profile debug --control-url "wss://control.example.com/control" --relay-url "https://relay.example.com"
```

Replace the deployment values before running. Output is under `.build/packages/`,
including `pixels-agent-bridge-windows-x86_64-debug-<version>-setup.exe` and checksum manifests.
Use complete packages for installation and upgrade testing. Release packaging needs
the matching Release binaries and explicit profile selection.

Each unified build allocates one product/installer version: first `1.2.0`, then one patch per
build, with `1.2.99 → 1.3.0` and `1.99.99 → 2.0.0`. Packaging does not increment
again. Every EXE/PKG installer filename includes its verified release version. Internal Rust, npm and Tauri versions remain unchanged for all builds. Cargo rebuilds changed inputs and reuses other cached artifacts; installer version allocation never edits component manifests or lockfiles.
See [BUILDING.md](BUILDING.md) for all targets and incremental build rules.

### Checks

The desktop Rust project is separate from the core Cargo workspace:

```sh
python scripts/verify_iroh_vendor.py
cargo test --locked --workspace
cargo test --locked --manifest-path apps/desktop/src-tauri/Cargo.toml --lib
cargo clippy --locked --workspace --all-targets -- -D warnings
```

Run `npx tsc --noEmit` from `apps/desktop` for frontend type validation without allocating a version. PostgreSQL integration
tests need a disposable test database through `DATABASE_URL`. Environment-dependent
and ignored tests require their documented setup. These are check commands, not a
claim that all platforms or optional tests have passed.

## Self-hosting

The supplied Compose stack runs PostgreSQL 17, the control backend, and Relay.

1. Copy `packaging/docker/example.env` to `packaging/docker/private.env`.
2. Configure the database password, DNS names, certificate paths, and port mappings.
3. Provision backend and Relay TLS certificates matching the DNS names, plus their
   CA files as required.
4. Create the private Relay control secret file specified by the environment file.

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
