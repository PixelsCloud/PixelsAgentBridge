# Development

The Server Web delivery and test contract is tracked in [WEB_DEVELOPMENT.md](WEB_DEVELOPMENT.md).
Remote task history is local to Bridge/Executor only. The server and Web do not
receive, retain, or display task history, command output, or task-derived statistics.

Pixels Agent Bridge has a first remotely operable vertical slice: an enrolled Bridge
can authenticate through the PostgreSQL control plane, reach an Executor through
iroh, execute an explicit native program, and stream persistent task events and
stdout/stderr back to the operator.
The Rust toolchain is pinned in `rust-toolchain.toml` and dependencies are locked in
`Cargo.lock`.

For fast desktop page work, run `npm run dev:desktop` from `apps/desktop`.
Tauri starts Vite on `http://localhost:1420` and opens a development window.
React and CSS edits update in that window without rebuilding Rust or making an
installer. The command uses `--no-watch` to avoid recompiling the backend while
editing pages; restart it after Rust changes. Use the complete Debug installer
only for final installed-package checks. Opening the Vite URL in a normal browser
does not provide Tauri's native commands or device state. Press Ctrl+Shift+I in
the development window to inspect its HTML, CSS, and console. Stop the process
with Ctrl+C when finished.

Run the current checks with:

```powershell
cargo fmt --all -- --check
python scripts/verify_iroh_vendor.py
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Development and test verification use Cargo's debug profile. Do not add
`--release` to routine checks or integration-test commands; release builds are
reserved for an explicit packaging or performance-validation step.

Keep implementation units divided by responsibility before they become costly to
compile or review. Protocol IDs, endpoint proof, Relay policy, and control messages
live in separate modules. Server TLS transport, connection-session state, and the
account, Team, and endpoint PostgreSQL repositories also remain separate. Split a
module when unrelated responsibilities or heavyweight dependencies start changing
together; create another crate only when the dependency graph or independent build
and test boundary justifies it.

`pab-protocol` also owns the serialized target-platform and task contracts shared by
Executor, Bridge, MCP, server, and UI adapters. `pab-task-runtime` is the small,
dependency-light state projection boundary. It enforces task transitions, monotonic
transfer progress and output ranges, and exact event-sequence recovery without
depending on networking, a database, platform process APIs, or a GUI. Adapters must
reuse this state model rather than defining their own task lifecycle. Every
device-facing Agent result must include the verified target context or its compact
reminder so the target OS, interpreter, path style, working directory, and environment
revision remain explicit after context compaction.

`pab-platform` is the isolated native platform boundary used by Executor builds. Its
first implementation detects the real Windows, macOS, or Linux OS version, CPU
architecture, path style, and process default working directory without launching a
shell. It produces a deterministic `native-v1` environment revision and leaves the
interpreter empty until a concrete command selects and verifies one. `os_info` is
exactly pinned with default features disabled; its target-specific system dependencies
do not enter the server, Relay, protocol, or task-runtime build graphs. A command's
requested working directory is task input and must not mutate the Executor process
working directory or silently redefine the published base environment.

`pab-transport` isolates the heavier pinned iroh 1.2.0 endpoint graph from protocol,
server, and task-runtime crates. It starts from iroh's `Minimal` preset, supplies the
registered endpoint key and the PAB ALPN, and requires at least one explicit HTTPS
Relay URL. It does not enable n0's public Relay or DNS address lookup. Normal embedded
root verification remains active and a deployment CA may be added; there is no
certificate-verification bypass. Optional iroh metrics, port mapping, and Apple's fast
datapath stay disabled until measurements justify their compile and runtime cost. UDP
hole punching and TLS Relay fallback remain available without those features.

PAB application messages use bounded length-prefixed JSON on reliable bidirectional
streams. A sender waits for QUIC acknowledgement before reporting delivery, so a
connection close cannot overtake a completed authentication result.

`pab-agent-core` is the GUI-independent client boundary shared by Windows, macOS,
and Linux Bridge/Executor processes. Its endpoint control handshake accepts only
`wss://`, uses normal certificate validation plus an optional private CA, caps control
frames at 64 KiB, validates every challenge identity field before signing, and keeps
the authenticated socket available for device control and ownership messages. Task history remains local and is never synchronized to Server. Its supervisor
requires matching pong heartbeats, retries indefinitely at a fixed three-second
interval after recoverable failures, increments a connection generation after every successful
authentication, and publishes current connection state through a Tokio watch channel
for UI/Executor adapters. The same receive loop multiplexes heartbeat, address, and
device-peer authorization responses. It does not persist account passwords or disable
certificate verification. Its enrollment API registers an account and initial
user/device Endpoints while checking every signed challenge field. Client challenge
validation tolerates at most 30 seconds of clock skew; the server still validates its
own issued challenge against its own clock.

`pab-bridge` is the GUI-independent operation-side connection boundary. Given a
previously registered user Endpoint, it authenticates WSS, starts the matching PAB
iroh Endpoint, queries an authorized device address, connects to the exact advertised
Endpoint ID, and verifies the device-session response against the configured user and
device. It can query the verified target environment, submit idempotent command
requests, query or cancel tasks, read output by byte offset, and subscribe from event
and output cursors. Its `BridgeRuntime` layer starts while offline, supervises the
control/iroh connection with a fixed three-second retry, reuses one authenticated
session per device, and resumes unfinished submissions and subscriptions from local
SQLite after restart. It persists the request before submission, then projects remote
snapshots, exact event cursors, and rolling 16 MiB stdout/stderr caches before
publishing bounded live events. Consumers recover from a lagged event channel by
querying SQLite. Every task event carries the target context, and an output retention
gap is explicitly reported before the cursor advances to the Executor's retained
boundary. The device password is supplied through a replaceable provider in a
zeroizing value; the Debug CLI reads it from a protected file and never accepts it as
a command or MCP argument. Both serialized password buffers and the Executor's
corresponding receive buffers are zeroized. A production desktop credential store
remains future work.

Device-code lookup only resolves identity. A validated network-address query
atomically registers or refreshes the operator/device Relay grant, so reconnecting
with a saved DeviceRef does not depend on a previous code lookup. Live Executor
sessions renew their grants during peer rechecks; the ten-minute expiry cleans up
leftover grants after sessions end. Bridge has no background code-lookup lease, and
MCP reuses resolved identities and authenticated connections. Address queries also
have a twenty-target limit per control session. Offline devices and permanent
server rejections fail explicitly instead of becoming generic connection timeouts.

Codex integration configures Pixels tools with `default_tools_approval_mode =
"approve"` and migrates existing per-tool overrides to `approve`, matching the
requested behavior of running all Pixels tools without approval prompts. Codex's
`auto` can still request approval based on tool annotations and therefore block
commands and uploads when approval prompts are disabled. Policy migration supports
regular and inline TOML tool tables while preserving unrelated server settings.
The two configuration regression tests pass; the current Codex session also
completed a forced installer upload to winserver and verified its remote SHA-256
using the registered `pixels.pab_*` tools without approval prompts.

The shared Bridge Host proposal was cancelled on 2026-09-30. Desktop and each
MCP process retain independent runtimes and remote connections. Desktop's main UI
process runs an Axum HTTP/WebSocket service on `0.0.0.0:26035`, without tokens or
authentication as requested. MCP connects to `ws://127.0.0.1:26035/ws/mcp` at startup
and reports process/client identity, tool calls, control status, used devices and
their codes/names/connection phases/P2P-or-Relay paths, plus session-scoped task and
operation/transfer summaries. HTTP `/health` and `/api/mcp` share the same listener.
Snapshots are kept in memory and delivered to React through Tauri query/events.
Reporting reconnects independently of tools when desktop is closed; a five-second
heartbeat and twenty-second timeout clean up abandoned connections. Complete
snapshots on reconnect and per-connection ownership prevent duplicate counts.
Passwords, keys, command arguments, output and remote-operation payloads are excluded.
Task facts remain in SQLite, with runtime/session ownership respected when reporting.
The implementation plan is `docs/desktop_mcp_reporting_2026-09-30.md` (local design
documents are intentionally ignored by Git). The service, reporting client and
Settings / AI Agent live process/device/task/operation view are implemented. The
tests cover independent connections, same-session replacement, timeout cleanup,
late startup/restart, session-scoped operations and output/argument exclusion.
A real stdio smoke test starts two `pab-mcp` processes, verifies registration before
tools, calls a local tool, then verifies forced and normal exit cleanup. Run it after
building `pab-mcp`, with `PAB_MCP_SMOKE_EXE` pointing to the new executable:
`cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --lib real_mcp_stdio_processes_register_before_tools_and_clean_up_on_exit -- --ignored`.

`pab-executor` is the first runnable, headless Executor entry point. It reads the
tenant, device, WSS URL, explicit self-hosted Relay URLs, endpoint-key
file, local device-credential file, and optional control/Relay private CAs from
environment configuration; secrets are accepted only from files. It binds an iroh
endpoint with the same registered key, detects the native environment, publishes
`DeviceHello`, reports supervised connection-state changes, reconnects, and closes
both connections on the platform termination signal. It watches iroh address changes
and publishes them through the authenticated control connection. It accepts bounded
incoming PAB connections and authenticates them before any task capability is exposed.
Each task uses a separate bounded iroh stream. The Executor runs the exact program and
argument vector without an implicit shell, drains stdout and stderr concurrently, and
stores acceptance, request deduplication, state, events, and output in local SQLite.
Unfinished tasks become `Interrupted` after restart. Output uses a rolling 16 MiB
retention window per stream, and an Executor runs at most 32 command tasks at once.
A subscription reports caught-up only after its event, stdout, and stderr cursors
agree with the same persisted snapshot.

Run the Debug Executor with a previously registered device endpoint:

```powershell
$env:PAB_TENANT_ID = "<tenant UUID>"
$env:PAB_DEVICE_ID = "<device UUID>"
$env:PAB_CONTROL_URL = "wss://server.example/control"
$env:PAB_RELAY_URLS = "https://relay-1.example,https://relay-2.example"
$env:PAB_ENDPOINT_SECRET_FILE = "C:\protected\pab-endpoint.key"
$env:PAB_TASK_DATABASE = "C:\protected\pab-executor.sqlite3"
# Optional; control and Relay may use different private CAs:
$env:PAB_CONTROL_CA_CERT = "C:\protected\pab-ca.pem"
$env:PAB_RELAY_CA_CERT = "C:\protected\relay-ca.pem"
cargo run -p pab-executor
```

The Debug command client prints the verified target OS reminder, streams output and
state, cancels on Ctrl+C, and retries recoverable connections indefinitely at the
fixed three-second interval. `follow` reconnects to an existing task and replays its
retained history:

```powershell
cargo run -p pab-bridge --bin pab-bridge -- command <9-digit-device-code> <program> [argument ...]
cargo run -p pab-bridge --bin pab-bridge -- follow <9-digit-device-code> <task-id>
```

Enrollment prints the nine-digit code and saves it in `executor.sqlite3`. The code
identifies a device within one server deployment; it grants no access by itself.
The Bridge checks the current workspace membership and device grant before resolving
it to the internal UUID. Resolution also works when the device is offline, so a
submitted task can wait for reconnection. The Debug CLI requires the nine-digit code.

The Bridge Runtime defaults to `bridge.sqlite3` in the persistent user data directory.
The Executor defaults to `device-endpoint.key` and `executor.sqlite3` in the
persistent machine data directory. The SQLite database holds the device code,
temporary password, password hash, and task history. `PAB_ENDPOINT_SECRET_FILE`,
`PAB_BRIDGE_DATABASE`, and `PAB_TASK_DATABASE` still
override individual paths; `PAB_DATA_DIR` overrides the data directory for either
process; set it to an absolute path outside the installation directory. SQLite creates
the parent directory when needed. The Bridge database contains
task metadata, events, cursors, and retained output; it never contains the device
password or endpoint private key.

| OS | Bridge user data | Executor machine data |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\PixelsAgentBridge` | `%PROGRAMDATA%\PixelsAgentBridge` |
| macOS | `~/Library/Application Support/PixelsAgentBridge` | `/Library/Application Support/PixelsAgentBridge` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/pixels-agent-bridge` | `/var/lib/pixels-agent-bridge` |

Keep the enrollment identity files and the Bridge device-password file in persistent
data storage, not beside installed binaries. Move the Executor identity and credential
files to its machine data directory on the remote computer. Reinstalling the program
must reuse these files and the existing server database; do not re-enroll an existing
device. Future uninstallers must remove binaries and services only, leaving these
data directories untouched unless the user separately requests a data reset.
The installer must create the machine directory with write access for the Executor
service account; Unix-created data directories use mode `0700`, while Windows
installer ACLs must restrict access to that service account and administrators.

Set `PAB_REQUEST_ID` to a caller-generated UUID when a durable caller must retry the
same submission across process restarts. Reusing it with different command input is
rejected.

For a fresh Debug deployment, prepare separate account and device password files, set
`PAB_CONTROL_URL` and `PAB_RELAY_URLS`, then create the initial
account, user Endpoint, and device Endpoint:

```powershell
cargo run -p pab-bridge --bin pab-enroll -- <username> <account-password-file> <device-name> <device-password-file> <new-output-directory>
```

The output directory argument is optional; without it, enrollment writes to the
persistent Bridge user data directory. The output directory is created before the
network request and must not contain any
generated filename. It receives two Endpoint key files, the local Argon2 device
credential, and a non-secret enrollment manifest. Keep the device Endpoint key and
credential on the Executor; keep the user Endpoint key and cleartext device password
on the Bridge side. The account password is used only during this enrollment flow.
Production onboarding and OS credential-store integration remain separate work.

The key file contains the 64 lowercase hexadecimal characters used by iroh's
`SecretKey` representation, with an optional trailing newline. Do not pass the key as
an environment value or print it in diagnostics.

The device-credential file is JSON with `schema_version: 1`, a positive
`password_version`, and an Argon2 PHC string in `password_hash`. The cleartext device
password is sent only inside the authenticated iroh connection, moved into a
zeroizing buffer on receipt, and never sent to the backend or Relay. A successful
authentication response contains identity and password-version facts but no reusable
session credential.

After a device endpoint authenticates, it publishes a versioned `DeviceHello` with
its immutable tenant/device reference and current execution context.
The server validates the endpoint principal and platform contract again, rechecks
that the device and endpoint are active, and stores the latest accepted environment
in PostgreSQL migration `0002_device_runtime.sql`. The connection supervisor republishes
the hello after every successful reconnect before reporting `Authenticated`, so task
submission can later use the stored environment revision as its expected-environment
guard. Device session messages, connection state/backoff, and control-session error
mapping stay in separate modules to keep each build unit focused.

The same authenticated session publishes a separate versioned device-network record.
`pab-transport` watches iroh address changes and exposes only HTTPS Relay URLs and
socket addresses; the lightweight protocol and server do not depend on iroh's address
types. Each Executor process uses a fresh instance ID and monotonically increasing
address revision. PostgreSQL migration `0003_device_network.sql` stores the latest
accepted address set only after rechecking the active device and endpoint key. The
steady-state control connection has one WebSocket receive loop for pongs, address
acknowledgements, and future server pushes, so independent features never compete for
frames.

An authenticated user endpoint may query a device-network snapshot only inside its
own tenant and only while its membership, endpoint, target device, target endpoint,
and device connect grant are active. Device registration creates the registering
user's initial grant. Team roles can manage grants but do not implicitly grant device
use. Missing and unauthorized snapshots share the same not-found result to avoid a
device-directory oracle. The returned endpoint key remains the identity that iroh
must authenticate when the Bridge connects.

For each incoming iroh session, the Executor takes the remote Endpoint ID from the
QUIC/TLS connection and asks its existing WSS supervisor to authorize that exact peer.
The server requires the user Endpoint to be online and rechecks the tenant membership,
device, both Endpoints, and the explicit device-connect grant in PostgreSQL. Current
online presence is process-local, so a single backend instance is the supported
topology for this phase. Before active-active backend deployment, connection routing
or shared presence coordination must replace this registry; Redis is optional and
does not become a permission source.

The `pab-server` crate owns the central PostgreSQL schema and control-plane services.
It does not expose an insecure HTTP listener or issue bearer tokens. Set
`PAB_DATABASE_URL` and use its initialization commands against an empty database:

```powershell
cargo run -p pab-server -- check
cargo run -p pab-server -- migrate
cargo run -p pab-server -- init
```

`init` is idempotent: it applies versioned schema files and creates the single
singleton server settings with the current 20/4/5 Mbps defaults if it is absent. PostgreSQL
integration tests use `DATABASE_URL`; SQLx creates and removes isolated test databases.
Use a disposable PostgreSQL instance with database-creation privileges for those tests.

Endpoint registration and reconnect authentication use a one-time proof signed by
the same Ed25519 secret key that produces the iroh Endpoint ID. Proof schema v2 binds
the connection, typed user-or-device principal, tenant, purpose, nonce,
and short validity window. A TLS-only WSS control service owns each proof session and
supports account registration, login, endpoint registration, and password-free
reconnects for active registered endpoints. It rechecks PostgreSQL after signature
verification so revoked endpoints and disabled owners fail authentication. Configure
`PAB_TLS_CERT`, `PAB_TLS_KEY`, and optionally
`PAB_LISTEN_ADDR` before running `cargo run -p pab-server -- serve`. The same
TLS listener exposes the internal Relay policy WSS endpoint. Set a random
`PAB_RELAY_CONTROL_SECRET` of at least 32 bytes on the server and provide the
same secret to each trusted Relay node; it is never sent outside TLS or logged.

The production Relay entry point is `cargo run -p pab-relay --bin pab-relay-server`.
It requires `PAB_CONTROL_URL` (a `wss://` URL),
`PAB_RELAY_CONTROL_SECRET`, `PAB_RELAY_TLS_CERT`, and `PAB_RELAY_TLS_KEY`.
`PAB_CONTROL_CA_CERT` adds trust for a self-signed control certificate. The Relay
defaults to HTTPS on `127.0.0.1:31443`, QUIC on `0.0.0.0:7842`, and keeps iroh's
captive portal on an automatically assigned loopback port. Public HTTPS and QUIC
bind addresses can be set with `PAB_RELAY_HTTPS_ADDR` and `PAB_RELAY_QUIC_ADDR`.

Relay nodes request a versioned full policy snapshot over the authenticated WSS
channel. An unchanged response extends the snapshot lifetime without resetting
rate buckets. A newer snapshot atomically replaces Endpoint ownership and removes
revoked entries. Unknown endpoints, cross-tenant forwarding, and expired policy
state fail closed. The production Relay service refreshes every 20 seconds. A cache
miss during endpoint admission or pair forwarding requests an immediate refresh over
the existing authenticated control channel, coalesced to at most one request per
second. New endpoint admission waits up to three seconds for that refresh; pair
forwarding drops unauthorized packets until the refreshed policy permits them, so
QUIC retransmission can complete the first handshake. It retries
the control connection indefinitely at a fixed three-second interval; the in-memory policy still expires if
the control service remains unavailable.

The Relay integration tests use loopback listeners and a generated self-signed
certificate. They verify explicit certificate trust, endpoint admission, and an
actual datagram transfer through `iroh-relay` 1.2.0. The plain HTTP captive-portal
listener is bound to loopback in this configuration; public deployments must expose
only the TLS and QUIC listeners.

`iroh-relay` is pinned to 1.2.0 and vendored only because PAB needs a forwarding hook
that receives both authenticated Endpoint IDs before applying aggregate limits. The
machine-readable baseline, reproducible patch, drift verifier, and upgrade checklist
live under `patches/iroh-relay`. The patch leaves iroh's captive portal unchanged; PAB
keeps that probe local and requires TLS on its own account, control, and data entrances.

Project design documents and local environment/server information are intentionally
excluded from Git.

The first Linux server container stack lives under `packaging/docker`. It builds the
backend and Relay with Cargo's Debug profile, uses persistent BuildKit caches for Rust
dependencies and build artifacts, runs both processes as an unprivileged user, and
mounts the Relay control credential as a Compose secret. PostgreSQL has no host port;
the two HTTPS listeners bind to host loopback for a reverse proxy, while Relay QUIC
publishes UDP 7842. See `packaging/docker/README.md` for the required certificate files
and startup commands.

The `pab-relay-probe` binary is a validation tool, not a production service. It can
start an allowlisted TLS Relay and exercise the public Relay protocol or QUIC address
discovery:

```powershell
cargo run --bin pab-relay-probe -- endpoint-id <32-byte-secret-hex>
cargo run --bin pab-relay-probe -- ping <https-relay-url> <secret-hex>
cargo run --bin pab-relay-probe -- qad <relay-ip:7842> <tls-server-name>
```

Run the binary without arguments to see the server, sender, and receiver forms. Probe
keys and host-specific deployment automation belong under the ignored `.env/`
directory and must not be committed.
