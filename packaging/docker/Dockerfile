FROM node:22-bookworm-slim AS web-builder
WORKDIR /web
COPY apps/web/package.json apps/web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY apps/web/ ./
RUN npm run build:assets

FROM rust:1.95-bookworm AS build-base
ENV CARGO_BUILD_JOBS=8 CMAKE_BUILD_PARALLEL_LEVEL=2

WORKDIR /src
COPY . .

FROM build-base AS management-builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --release \
        -p pab-server --bin pab-server \
    && mkdir -p /out \
    && cp /src/target/release/pab-server /out/pab-server

FROM build-base AS relay-builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --locked --release \
        -p pab-relay --bin pab-relay-server --bin pab-relay-healthcheck \
    && mkdir -p /out \
    && cp /src/target/release/pab-relay-server /out/pab-relay-server \
    && cp /src/target/release/pab-relay-healthcheck /out/pab-relay-healthcheck

FROM debian:bookworm-slim AS runtime-base

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system pab \
    && useradd --system --gid pab --home-dir /nonexistent --shell /usr/sbin/nologin pab \
    && install -d -m 0700 -o pab -g pab /var/log/pab /var/lib/pab/usage

COPY LICENSE NOTICE COMMERCIAL_LICENSE.md /usr/share/doc/pixels-agent-bridge/
USER pab:pab
ENTRYPOINT []

FROM runtime-base AS management
ARG PAB_RELEASE_VERSION=development
ARG PAB_RELEASE_COMMIT=unknown
ARG PAB_SOURCE_SHA256=unknown
LABEL vip.rgaa.pab.product="management" \
      org.opencontainers.image.version=$PAB_RELEASE_VERSION \
      org.opencontainers.image.revision=$PAB_RELEASE_COMMIT \
      vip.rgaa.pab.source-sha256=$PAB_SOURCE_SHA256
COPY --from=management-builder /out/pab-server /usr/local/bin/pab-server
COPY --from=web-builder /web/dist /usr/local/bin/web
EXPOSE 8443/tcp
CMD ["pab-server", "serve", "--config", "/etc/pab/pab-server.toml"]

FROM runtime-base AS relay
ARG PAB_RELEASE_VERSION=development
ARG PAB_RELEASE_COMMIT=unknown
ARG PAB_SOURCE_SHA256=unknown
LABEL vip.rgaa.pab.product="relay" \
      org.opencontainers.image.version=$PAB_RELEASE_VERSION \
      org.opencontainers.image.revision=$PAB_RELEASE_COMMIT \
      vip.rgaa.pab.source-sha256=$PAB_SOURCE_SHA256
COPY --from=relay-builder /out/pab-relay-server /usr/local/bin/pab-relay-server
COPY --from=relay-builder /out/pab-relay-healthcheck /usr/local/bin/pab-relay-healthcheck
EXPOSE 31443/tcp 7842/udp
CMD ["pab-relay-server", "--config", "/etc/pab/pab-relay-server.toml"]

# Compatibility target for existing builds and deployments.
FROM runtime-base AS runtime
COPY --from=management-builder /out/pab-server /usr/local/bin/pab-server
COPY --from=relay-builder /out/pab-relay-server /usr/local/bin/pab-relay-server
COPY --from=relay-builder /out/pab-relay-healthcheck /usr/local/bin/pab-relay-healthcheck
COPY --from=web-builder /web/dist /usr/local/bin/web
EXPOSE 8443/tcp 31443/tcp 7842/udp
CMD ["pab-server", "serve", "--config", "/etc/pab/pab-server.toml"]
