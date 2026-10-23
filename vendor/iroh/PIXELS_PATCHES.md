# Pixels transport policy extension

Based on iroh 1.2.0 from the crates.io source distribution. Upstream license files
are retained. The application needs a production per-device Relay-only choice;
removing IP transports from the shared endpoint would affect unrelated devices.

`ConnectOptions::with_relay_only` propagates a policy to the remote endpoint's
actor before starting the handshake. It excludes direct initial packets even
when addresses are cached, suppresses hole punching and local NAT candidates,
prevents opening direct paths, and restricts subsequent selection to Relay.
The application closes a remote's existing connection before changing its mode.
Other remote actors keep the default path selection and IP transports.

Coverage is in `crates/relay/tests/cluster_transport.rs`, using real TLS relays
and locally reachable IP transports, including mode changes and peer isolation.

The `iroh` and `iroh-relay` Cargo libraries emit Rust libraries only. The
application does not consume an FFI dynamic library from either crate. Emitting
both `lib` and `cdylib` creates an unhashed rlib that can be overwritten when the
root and standalone Desktop build graphs share a target directory. Removing
the unused dynamic output preserves Cargo's feature-specific library artifacts.
