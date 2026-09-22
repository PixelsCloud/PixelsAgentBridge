FROM rust:1.95-bookworm AS builder

WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --profile dev \
        -p pab-server --bin pab-server \
        -p pab-relay --bin pab-relay-server \
    && mkdir -p /out \
    && cp /src/target/debug/pab-server /out/pab-server \
    && cp /src/target/debug/pab-relay-server /out/pab-relay-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system pab \
    && useradd --system --gid pab --home-dir /nonexistent --shell /usr/sbin/nologin pab

COPY --from=builder /out/pab-server /usr/local/bin/pab-server
COPY --from=builder /out/pab-relay-server /usr/local/bin/pab-relay-server

USER pab:pab
EXPOSE 8443/tcp 31443/tcp 7842/udp
ENTRYPOINT []
CMD ["pab-server", "serve"]
