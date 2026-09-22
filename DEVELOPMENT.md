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
