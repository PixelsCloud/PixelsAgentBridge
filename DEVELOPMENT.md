# Development

Pixels Agent Bridge is currently validating its Relay, traffic-control, and PostgreSQL
control-plane foundations.
The Rust toolchain is pinned in `rust-toolchain.toml` and dependencies are locked in
`Cargo.lock`.

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

`pab-agent-core` is the GUI-independent client boundary shared by Windows, macOS,
and Linux Bridge/Executor processes. Its endpoint control handshake accepts only
`wss://`, uses normal certificate validation plus an optional private CA, caps control
frames at 64 KiB, validates every challenge identity field before signing, and keeps
the authenticated socket available for later task-sync protocols. Its supervisor
requires matching pong heartbeats, reconnects with bounded identity-jittered
exponential backoff, increments a connection generation after every successful
authentication, and publishes current connection state through a Tokio watch channel
for UI/Executor adapters. It does not persist account passwords or disable certificate
verification.

After a device endpoint authenticates, it publishes a versioned `DeviceHello` with
its immutable deployment/tenant/device reference and current execution context.
The server validates the endpoint principal and platform contract again, rechecks
that the device and endpoint are active, and stores the latest accepted environment
in PostgreSQL migration `0002_device_runtime.sql`. The connection supervisor republishes
the hello after every successful reconnect before reporting `Authenticated`, so task
submission can later use the stored environment revision as its expected-environment
guard. Device session messages, connection state/backoff, and control-session error
mapping stay in separate modules to keep each build unit focused.

The `pab-server` crate owns the central PostgreSQL schema and control-plane services.
It does not expose an insecure HTTP listener or issue bearer tokens. Set
`PAB_DATABASE_URL` and use its initialization commands against an empty database:

```powershell
cargo run -p pab-server -- check
cargo run -p pab-server -- migrate
cargo run -p pab-server -- init
```

`init` is idempotent: it applies versioned schema files and creates the single
deployment record with the current 20/4/5 Mbps defaults if it is absent. PostgreSQL
integration tests use `DATABASE_URL`; SQLx creates and removes isolated test databases.
Use a disposable PostgreSQL instance with database-creation privileges for those tests.

Endpoint registration and reconnect authentication use a one-time proof signed by
the same Ed25519 secret key that produces the iroh Endpoint ID. Proof schema v2 binds
the deployment, connection, typed user-or-device principal, tenant, purpose, nonce,
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
It requires `PAB_DEPLOYMENT_ID`, `PAB_CONTROL_URL` (a `wss://` URL),
`PAB_RELAY_CONTROL_SECRET`, `PAB_RELAY_TLS_CERT`, and `PAB_RELAY_TLS_KEY`.
`PAB_CONTROL_CA_CERT` adds trust for a self-signed control certificate. The Relay
defaults to HTTPS on `127.0.0.1:31443`, QUIC on `0.0.0.0:7842`, and keeps iroh's
captive portal on an automatically assigned loopback port. Public HTTPS and QUIC
bind addresses can be set with `PAB_RELAY_HTTPS_ADDR` and `PAB_RELAY_QUIC_ADDR`.

Relay nodes request a versioned full policy snapshot over the authenticated WSS
channel. An unchanged response extends the snapshot lifetime without resetting
rate buckets. A newer snapshot atomically replaces Endpoint ownership and removes
revoked entries. Unknown endpoints, cross-tenant forwarding, and expired policy
state fail closed. The production Relay service refreshes every 20 seconds and
reconnects with bounded exponential backoff; the in-memory policy still expires if
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
