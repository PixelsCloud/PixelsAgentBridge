# Development

Pixels Agent Bridge is currently validating its Relay and traffic-control foundations.
The Rust toolchain is pinned in `rust-toolchain.toml` and dependencies are locked in
`Cargo.lock`.

Run the current checks with:

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

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
cargo run --release --bin pab-relay-probe -- endpoint-id <32-byte-secret-hex>
cargo run --release --bin pab-relay-probe -- ping <https-relay-url> <secret-hex>
cargo run --release --bin pab-relay-probe -- qad <relay-ip:7842> <tls-server-name>
```

Run the binary without arguments to see the server, sender, and receiver forms. Probe
keys and host-specific deployment automation belong under the ignored `.env/`
directory and must not be committed.
