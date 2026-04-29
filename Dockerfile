# syntax=docker/dockerfile:1.7

# ----------------------------------------------------------------------------
# Fennec container build
# ----------------------------------------------------------------------------
#
# Two-stage build:
#
#   1. `builder` — full Rust toolchain on Debian Bookworm. Compiles the
#      binary and cleans up.
#   2. `runtime` — minimal Debian Bookworm slim image with just the binary,
#      its runtime deps (CA roots, libssl3 in case any transitive dep
#      pulls OpenSSL), and the shipped skills/ tree.
#
# The build stage uses a cargo "fetch deps first, then source" trick: copying
# Cargo.toml + Cargo.lock + a stub main.rs lets `cargo build --release` cache
# the dependency graph in a separate layer. Subsequent rebuilds that only
# touch src/ skip the long dep compile.
#
# The runtime image runs as a non-root user (`fennec`, uid 10001) with
# FENNEC_HOME=/data. /data is declared as a VOLUME so config, secrets, and
# memory survive container recreation. The entrypoint seeds the in-tree
# skills/ into /data/skills on first boot if it isn't already present —
# this preserves user customisations across image upgrades.
#
# Default command is `gateway` listening on 0.0.0.0:3000. Override with:
#   docker run --rm fennec:latest agent --message "hello"
# or by setting `command:` in docker-compose.yml.

# ----------------------------------------------------------------------------
# Stage 1: builder
# ----------------------------------------------------------------------------
FROM rust:1.87-slim-bookworm AS builder

# Build deps. pkg-config + libssl-dev cover any transitive dep that needs
# OpenSSL headers; the Fennec dep tree itself uses rustls-tls so this is
# defence in depth, not a hard requirement.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# ---- Step 1a: dep cache layer ----
# Copy only the manifest + lockfile, write a stub main.rs, and build. The
# resulting target/release/deps is then frozen into a Docker layer that
# stays valid until Cargo.toml or Cargo.lock changes.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() { println!("stub"); }' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src target/release/fennec target/release/fennec.d

# ---- Step 1b: real source ----
COPY src/ ./src/
COPY skills/ ./skills/

# `--locked` matches CI: fail loudly if Cargo.lock is out of sync. `strip`
# trims a few MB of debug symbols off the release binary; the existing
# `[profile.release]` section already does this in some toolchains but
# we run it explicitly to be sure.
RUN cargo build --release --locked --bin fennec \
    && strip target/release/fennec

# ----------------------------------------------------------------------------
# Stage 2: runtime
# ----------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 fennec \
    && useradd --system --uid 10001 --gid fennec --create-home --home-dir /home/fennec fennec

# Binary + in-tree skills.
COPY --from=builder /build/target/release/fennec /usr/local/bin/fennec
COPY --from=builder /build/skills /opt/fennec/skills

# Entrypoint that seeds skills on first boot.
COPY docker/entrypoint.sh /usr/local/bin/fennec-entrypoint
RUN chmod 0755 /usr/local/bin/fennec-entrypoint

# /data holds config.toml, .key, memory.db, cron_jobs.json, snapshots, etc.
# It's declared as a volume so docker-compose / `docker run -v` will preserve
# it. FENNEC_HOME points the binary at it; the install path's `~/.fennec`
# convention applies inside the container at `/data` instead.
ENV FENNEC_HOME=/data \
    RUST_LOG=info

# Pre-create /data with correct ownership so the entrypoint can write to it
# even before the operator's volume mount fully propagates. If a host
# bind-mount is used, the host path's ownership wins — see the README's
# Docker section for the chown pattern.
RUN install -d -o fennec -g fennec -m 0700 /data

USER fennec
WORKDIR /data
VOLUME ["/data"]

# Default gateway port. Override by passing `--port` to the gateway command.
EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/fennec-entrypoint"]
CMD ["gateway", "--host", "0.0.0.0", "--port", "3000"]
