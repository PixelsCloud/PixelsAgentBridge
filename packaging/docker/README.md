# Linux Docker server prototype

This Compose stack runs PostgreSQL 17, the TLS control backend, and the TLS/QUIC
iroh Relay. The image is compiled with Cargo's stripped Release profile.

The backend also serves the built React/Ant Design Web console on the same HTTPS
origin. Set `PAB_WEB_ORIGIN` to the public origin and configure the proxy for WebSocket
upgrades. Provision an existing account with `pab-server web-admin <username>`.
See [Web deployment and rollback](../../WEB_DEPLOYMENT.md). Task history stays local;
Server/Web neither receive nor display it.

`PAB_RELAY_IMAGE` may override just the Relay image for an independent update;
otherwise it uses `PAB_SERVER_IMAGE` like the backend.

PostgreSQL uses the named `postgres-data` volume, so replacing images or running
`docker compose down` keeps accounts, device UUID/code mappings, and grants. Do not
use `docker compose down -v` for an ordinary uninstall or upgrade; removing that
volume is an explicit data reset.

Copy `example.env` to a private env file and configure
the control and Relay URLs. Create a random Relay control
secret containing at least 32 bytes at the path named by
`PAB_RELAY_CONTROL_SECRET_PATH`.

The certificate directory must contain:

- `backend-cert.pem`, `backend-key.pem`, and `backend-ca.pem`
- `relay-cert.pem`, `relay-key.pem`, and `relay-ca.pem`

The backend certificate DNS SAN must match `PAB_BACKEND_TLS_NAME`; the Relay
certificate DNS SAN must match `PAB_RELAY_TLS_NAME`. For testing, create a local
CA, use it to sign both server leaf certificates, and put the corresponding CA
certificate in each `*-ca.pem` file. Do not mark a server leaf certificate as a CA.

Build and start the stack from the repository root:

```sh
python scripts/build.py docker --profile release
# Set PAB_SERVER_IMAGE (and any explicit PAB_RELAY_IMAGE) in the private env
# file to pixels-agent-bridge:<version printed by the build>.
docker compose --env-file packaging/docker/private.env \
  -f packaging/docker/compose.yaml up -d --no-build
docker compose --env-file packaging/docker/private.env \
  -f packaging/docker/compose.yaml ps
```

The backend HTTPS and Relay HTTPS ports bind to host loopback by default for an
existing reverse proxy. Relay QUIC publishes UDP 7842 directly. The upstream
iroh captive-portal listener stays inside the Relay container on loopback and is
never published by Compose.

Use the unified entry point, not `docker compose build`, so each build allocates
one product version before Docker copies the sources. See [BUILDING.md](../../BUILDING.md).

The Relay control secret is mounted through a Compose secret file. PostgreSQL is
reachable only on the private Compose network. No public HTTP listener is added.
Backend and Relay logs are stored in separate persistent named volumes at
`/var/log/pab`. Each process keeps at most five 16 MiB files. Set
`PAB_LOG_LEVEL` for a different event filter.
BuildKit keeps Cargo registry, git, and target caches between builds so a source or
packaging change does not force all third-party Rust crates to compile again.
