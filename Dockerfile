# syntax=docker/dockerfile:1
#
# Server image for the enterprise deployment (dream-en's `deploy/`):
# `dreamcore` (the main server, `enterprise` feature on) and
# `dreamcore-admin` (the governance-plane binary — see
# `crates/dream-core-app/src/bin/admin.rs`) in one image, run as two
# containers from the same `docker-compose.yml` with different `command:`.
# This is NOT the desktop-bundled image dream-ui ships (that one wraps
# `aioncore` in the `aionui-web` launcher and vendors managed agent-runtime
# binaries via a separate cross-repo packaging step,
# `dream-ui/scripts/pack-web-cli.js`) — this one compiles straight from this
# repo and ships the two bare binaries.
#
#   docker build -t dream-core-enterprise .
#
# Build from source rather than reusing a staged tarball: unlike dream-ui,
# this repo has no separate CI packaging step that produces one, and the
# workspace has no non-Rust build steps to run first.
#
# NOT included here, and why that's fine for this deployment:
# - Managed agent-runtime / CLI binaries (Node, Claude Code, etc.): dreamcore
#   defaults to `--managed-resources-mode download` (see
#   `crates/dream-core-app/src/cli.rs`), so a network-connected container
#   fetches what it needs on first use. `dreamcore-admin` never spawns an
#   agent at all — see its own module doc.
# - `officecli` (PPTX/DOCX conversion helper, a separate .NET binary iOfficeAI
#   ships): `dream-core-office` calls out to it only if configured/found and
#   returns a clean "officecli not installed" error otherwise
#   (`crates/dream-core-office/src/conversion.rs`) — it does not crash
#   startup or link against it. Bundling it is future work if this
#   deployment needs Office-file conversion.

FROM rust:1.95-bookworm AS builder

# git: `.cargo/config.toml` sets `net.git-fetch-with-cli = true` (needed for
# the `dream-engine-*` git dependencies below — the `rust:*` image does not
# ship git by default). dream-engine is public
# (github.com/gaogg521/dream-engine), so this clones anonymously; no
# credentials to inject.
RUN apt-get update && apt-get install -y --no-install-recommends git && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Workspace manifests first so `cargo fetch` is cacheable independently of
# source edits — every crate's Cargo.toml, since this is a workspace.
COPY Cargo.toml Cargo.lock ./
COPY .cargo .cargo
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked --features enterprise -p dream-core-app \
    && cp target/release/dreamcore /build/dreamcore \
    && cp target/release/dreamcore-admin /build/dreamcore-admin
# The cache-mounted target/ dir is gone once this RUN layer ends, hence
# copying the two binaries out to a plain (non-cache-mounted) location first.

FROM debian:bookworm-slim

# ca-certificates: both binaries call out to model provider APIs over HTTPS
# (reqwest with rustls + native root certs — see the workspace Cargo.toml —
# reads the OS trust store, it does not vendor one).
# curl: used by the HEALTHCHECK below.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app --shell /usr/sbin/nologin dreamcore

WORKDIR /app
COPY --from=builder /build/dreamcore /build/dreamcore-admin ./
RUN chmod +x ./dreamcore ./dreamcore-admin

# 0.0.0.0, not the CLI's loopback default: a container's loopback is its own
# network namespace, unreachable from a sibling container (the gateway) even
# on the same Docker network. See the matching comment in
# dream-en/deploy/docker-compose.yml.
ENV AIONUI_HOST=0.0.0.0 \
    AIONUI_PORT=25808 \
    AIONUI_ADMIN_PORT=25809 \
    AIONUI_DATA_DIR=/data

# SQLite database + logs + workspace files, shared between `dreamcore` and
# `dreamcore-admin` when both run against this image (E3 shared-database
# deployment) — mount the same volume/bind for both containers, never two
# separate ones.
#
# `/data` must be created and owned by `dreamcore` *before* `VOLUME` and
# `USER` below: for a named volume (what docker-compose.yml uses), Docker
# seeds it from whatever the image already has at this path on first mount,
# permissions included — do this after switching USER and an empty root-owned
# volume shows up instead, which the non-root process below can't write to.
RUN mkdir -p /data && chown dreamcore:dreamcore /app /data
VOLUME ["/data"]
USER dreamcore

EXPOSE 25808 25809

# `/health` is unauthenticated and answers as soon as the router is up (both
# binaries mount it) — see `crates/dream-core-app/src/router/health.rs`. One
# image, two possible `command:`s (see the top-of-file comment) means this
# same HEALTHCHECK has to work for whichever binary this particular container
# is actually running: try both ports, whichever one nothing is listening on
# just fails its own `curl` harmlessly.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${AIONUI_PORT}/health" || curl -fsS "http://127.0.0.1:${AIONUI_ADMIN_PORT}/health"

ENTRYPOINT ["./dreamcore"]
