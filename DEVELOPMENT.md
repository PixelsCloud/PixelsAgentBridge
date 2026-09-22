# Development

Pixels Agent Bridge is currently validating its Relay, traffic-control, and PostgreSQL
control-plane foundations.
The Rust toolchain is pinned in `rust-toolchain.toml` and dependencies are locked in
`Cargo.lock`.

Run the current checks with:

```powershell
cargo fmt --all -- --check
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

Endpoint registration accepts a one-time proof signed by the same Ed25519 secret key
that produces the iroh Endpoint ID. The proof binds the deployment, connection,
account, tenant, purpose, nonce, and short validity window. A TLS-only WSS control
service now owns each proof session and supports account registration, login, and
endpoint registration. Configure `PAB_TLS_CERT`, `PAB_TLS_KEY`, and optionally
`PAB_LISTEN_ADDR` before running `cargo run -p pab-server -- serve`. The same
TLS listener exposes the internal Relay policy WSS endpoint. Set a random
`PAB_RELAY_CONTROL_SECRET` of at least 32 bytes on the server and provide the
same secret to each trusted Relay node; it is never sent outside TLS or logged.

Relay nodes request a versioned full policy snapshot over the authenticated WSS
channel. An unchanged response extends the snapshot lifetime without resetting
rate buckets. A newer snapshot atomically replaces Endpoint ownership and removes
revoked entries. Unknown endpoints, cross-tenant forwarding, and expired policy
state fail closed. The production Relay service loop still needs to schedule these
refreshes and reconnect the channel before packaging.

The Relay integration tests use loopback listeners and a generated self-signed
certificate. They verify explicit certificate trust, endpoint admission, and an
actual datagram transfer through `iroh-relay` 1.2.0. The plain HTTP captive-portal
listener is bound to loopback in this configuration; public deployments must expose
only the TLS and QUIC listeners.

Project design documents and local environment/server information are intentionally
excluded from Git.

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
