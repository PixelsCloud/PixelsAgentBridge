FROM rust:1.95-bookworm AS builder

WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release \
        -p pab-server --bin pab-server \
        -p pab-relay --bin pab-relay-server \
    && mkdir -p /out \
    && cp /src/target/release/pab-server /out/pab-server \
    && cp /src/target/release/pab-relay-server /out/pab-relay-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system pab \
    && useradd --system --gid pab --home-dir /nonexistent --shell /usr/sbin/nologin pab \
    && install -d -m 0700 -o pab -g pab /var/log/pab

COPY --from=builder /out/pab-server /usr/local/bin/pab-server
COPY --from=builder /out/pab-relay-server /usr/local/bin/pab-relay-server

USER pab:pab
ENV PAB_LOG_DIR=/var/log/pab
EXPOSE 8443/tcp 31443/tcp 7842/udp
ENTRYPOINT []
CMD ["pab-server", "serve"]
