# ── Stage 1: Builder ──────────────────────────────────────────────────────────
FROM rust:1.88-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

ENV OPENSSL_NO_VENDOR=1

WORKDIR /build

# Workspace manifest includes the crates needed to build the merged app binary.
# tests/ and cli/ are not built inside the image — they're for local use only.
RUN printf '[workspace]\nmembers = ["core", "app", "k8s-ops"]\nresolver = "2"\n' > Cargo.toml

COPY Cargo.lock ./
COPY core/Cargo.toml    core/
COPY app/Cargo.toml     app/
COPY k8s-ops/Cargo.toml k8s-ops/

# Stub sources for dep-caching layer.
RUN mkdir -p core/src app/src k8s-ops/src && \
    echo '' > core/src/lib.rs && \
    echo 'fn main() {}' > app/src/main.rs && \
    echo '' > k8s-ops/src/lib.rs

RUN cargo fetch

COPY core/src    core/src
COPY app/src     app/src
COPY k8s-ops/src k8s-ops/src

RUN cargo build --release -p octo-app

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates curl git gpg && \
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        | gpg --dearmor -o /usr/share/keyrings/githubcli-archive-keyring.gpg && \
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list && \
    apt-get update && \
    apt-get install -y \
    awscli \
    gh \
    jq \
    nodejs \
    qrencode \
    && rm -rf /var/lib/apt/lists/* && \
    # uvx (via uv)
    curl -LsSf https://astral.sh/uv/install.sh | UV_INSTALL_DIR=/usr/local/bin sh

COPY --from=builder /build/target/release/octo-app /usr/local/bin/octo-app

LABEL org.opencontainers.image.source=https://github.com/georgebradford0/octo

ENV HOME=/root
ENV OCTO_SKIP_SHELL_ENV=1
ENV OCTO_DATA_DIR=/data
ENV NOISE_KEY_FILE=/data/noise_key.bin

# Default ENTRYPOINT runs the parent (lair) role. Child Deployments override
# this with `command: ["/usr/local/bin/octo-app", "--role", "server"]`.
ENTRYPOINT ["/usr/local/bin/octo-app", "--role", "lair"]
