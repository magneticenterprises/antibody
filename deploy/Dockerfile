# syntax=docker/dockerfile:1.7
# Antibody Rust services: antibody-api and antibody-worker in one image.
# Build from the repository root:
#   docker build -f deploy/Dockerfile -t antibody:dev .

FROM rust:1.97-slim-trixie AS builder
WORKDIR /src

# Cargo registry and build artefacts are cached across builds with BuildKit
# cache mounts; only the final binaries are copied out of the cache.
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p antibody-api -p antibody-worker \
    && mkdir -p /out \
    && cp target/release/antibody-api target/release/antibody-worker /out/

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 antibody \
    && useradd --system --uid 10001 --gid antibody --no-create-home --shell /usr/sbin/nologin antibody

COPY --from=builder /out/antibody-api /out/antibody-worker /usr/local/bin/

USER antibody:antibody
ENV ANTIBODY_BIND=0.0.0.0:8080
EXPOSE 8080
# Default command serves the API and MCP endpoints. Compose overrides it with
# `antibody-worker` for the worker container.
CMD ["antibody-api", "serve"]
