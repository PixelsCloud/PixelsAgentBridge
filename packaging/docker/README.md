# Server and Relay deployment

Each executable reads **one TOML file**. Compose handles containers, mounts,
networking and PostgreSQL bootstrap. Desktop, Executor and MCP are unchanged.

| Executable | Configuration | Contents |
| --- | --- | --- |
| `pab-server` | `pab-server.toml` | Database, HTTPS/TLS, Web, registration, GitHub, Relay control secret, logs |
| `pab-relay-server` | `pab-relay-server.toml` | Node ID, usage storage, HTTPS/QUIC/TLS, Server control connection, logs |

Without `--config`, the file is read from the working directory. Relative paths
inside TOML resolve against the configuration file's directory. Unknown fields
and malformed values fail startup without printing secrets. Old service `PAB_*`
environment variables and separate GitHub JSON files are no longer read.

```sh
pab-server init --config /etc/pab/pab-server.toml
pab-server serve --config /etc/pab/pab-server.toml
pab-server web-admin <existing-username> --config /etc/pab/pab-server.toml
pab-relay-server --config /etc/pab/pab-relay-server.toml
```

## Configure and start

1. Copy `example.env` to `private.env`: deployment settings only (images, host
   paths, published ports, network alias, PostgreSQL bootstrap credentials).
2. Create `config/`. Copy both `.example.toml` files there, removing `.example`
   from the filenames. Set Server `database.url` to match PostgreSQL bootstrap
   credentials. Percent-encode reserved characters in URL credentials.
3. Set Server `web.origin` to the public HTTPS origin, and TLS/Web asset paths
   to paths visible **inside the container**.
4. Generate a random secret of at least 32 printable ASCII characters. Put the
   same value in Server `relay.control_secret` and Relay `control.secret`.
5. Set Relay `control.url` to Server's WSS endpoint, with matching DNS and TLS
   certificate SAN. The sample uses `backend`; alternatively set the Compose
   network alias and URL host to your existing certificate's name. Set
   `control.ca_cert` for a private CA; omit for public CA trust.
6. Mount `backend-cert.pem`, `backend-key.pem`, `backend-ca.pem`,
   `relay-cert.pem`, `relay-key.pem`, `relay-ca.pem` from the certificate directory. Certificates
   must match the names used by clients, reverse proxy and Compose health checks.
   Set the two Compose DNS aliases to names present in their certificate SANs.
7. Optionally enable `[github]` in Server TOML. See [GitHub setup](../../GITHUB_LOGIN_SETUP.md).

Real TOMLs and private directories are excluded from Git and Docker's build
context. Restrict permissions while allowing the container's `pab` user to read
its file. Each service mounts **only its own** TOML read-only; GitHub secrets are
not mounted into Relay. Configuration is never copied into the image.

```sh
python scripts/build.py docker --profile release
# Set PAB_SERVER_IMAGE in private.env to the version printed by the build.
# PAB_RELAY_IMAGE may select an independently updated Relay image.
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d --no-build
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml ps
```

After editing TOML, recreate the affected container to remount atomically replaced
files. Changing the shared secret requires updating both services. Configuration
edits do not require rebuilding images:

```sh
docker compose --env-file packaging/docker/private.env -f packaging/docker/compose.yaml up -d --no-deps --force-recreate backend
```

The GitHub Compose overlay has been removed. Before deploying the new binaries,
convert existing private environment/JSON values to these TOMLs, preserving the
database URL, secrets, certificates and GitHub proxy. No database reset or client
reinstall is needed for this change.

## Account bandwidth

Every initiating Desktop/MCP must sign in. Receiving devices can run unattended
without account login; the device password is still required.

New accounts receive **10 Mbps**. Web administrators choose an account limit from
5, 10, 20, 30, 40, 50, 60, 70, 80, 90 or 100 Mbps. There are no guest quotas,
service-wide bandwidth settings or TOML bandwidth keys. Remove `default_user_mbps`
and `default_guest_mbps` from existing private server TOMLs before upgrading.
Server and Relay must be upgraded together (policy schema 6).

Units are decimal **Mbps**, not MB/s: 10 Mbps = 1.25 MB/s before overhead.

Forwarding uses a token bucket at `Mbps * 1_000_000 / 8` bytes/second with 100 ms
of burst credit. Excess traffic waits. All MCP connections for a user share one
bucket, including uploads and downloads. Unauthenticated operator endpoints cannot forward traffic. P2P
traffic bypasses Relay limits. Buckets are per Relay node, not cluster-wide.

## Persistence and health

PostgreSQL's `postgres-data` volume preserves accounts and device identities.
Do not use `docker compose down -v` for ordinary updates. Usage and Server/Relay
logs have separate persistent volumes. Each process keeps five 16 MiB log files;
set `[log] level` to change the tracing filter.

The example publishes HTTPS to host loopback for a TLS-aware reverse proxy and
QUIC over UDP 7842. Container health checks probe loopback readiness with
CA/SAN verification using the configured network aliases. Client, proxy and Relay
control connections also verify TLS. The iroh captive
portal stays on an automatically assigned loopback port.

BuildKit retains Cargo caches. Use the [build entry point](../../BUILDING.md) for
versioned incremental builds. See [Web deployment](../../WEB_DEPLOYMENT.md) for
proxy setup. Task history stays local; Server/Web neither receive nor display it.
