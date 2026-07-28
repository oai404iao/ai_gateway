# syntax=docker/dockerfile:1.7

ARG NODE_VERSION=24
ARG RUST_VERSION=1.97.1
ARG RUST_IMAGE_DIGEST=sha256:77fac8b98f9f46062bb680b6d25d5bcaabfc400143952ebc572e924bcbedc3fa
ARG CARGO_CHEF_VERSION=0.1.73
ARG CARGO_CHEF_RUST_VERSION=1.92.0
ARG CARGO_CHEF_DIGEST=sha256:856a5a208a9d33cf1eaddb9b78e67192a6dd3381f8299f0c5880b44f15e271ea

# Static Console assets are architecture-independent; build them natively.
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-bookworm-slim AS console-builder
ARG PNPM_VERSION=11.17.0
ENV PNPM_HOME=/pnpm
ENV PATH="${PNPM_HOME}:${PATH}"
WORKDIR /workspace

RUN npm install --global "pnpm@${PNPM_VERSION}"
COPY web/console/package.json web/console/pnpm-lock.yaml web/console/pnpm-workspace.yaml ./web/console/
RUN --mount=type=cache,target=/pnpm/store \
    pnpm config set store-dir /pnpm/store \
    && pnpm --dir web/console install --frozen-lockfile
COPY web/console ./web/console
RUN pnpm --dir web/console build \
    && pnpm --dir web/console list --prod --depth Infinity --json \
        > web/console/production-dependencies.json

# Use cargo-chef only as an architecture-matched binary source. Project
# dependency and release compilation both run in the pinned official Rust image.
FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:${CARGO_CHEF_VERSION}-rust-${CARGO_CHEF_RUST_VERSION}-bookworm@${CARGO_CHEF_DIGEST} AS cargo-chef-planner
FROM lukemathwalker/cargo-chef:${CARGO_CHEF_VERSION}-rust-${CARGO_CHEF_RUST_VERSION}-bookworm@${CARGO_CHEF_DIGEST} AS cargo-chef-builder

# The cargo-chef recipe is architecture-independent; prepare it natively.
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-bookworm@${RUST_IMAGE_DIGEST} AS planner
WORKDIR /workspace
# The exact image already contains RUST_VERSION; this override prevents the
# repository toolchain file from requesting development-only components.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
COPY --from=cargo-chef-planner /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM rust:${RUST_VERSION}-bookworm@${RUST_IMAGE_DIGEST} AS builder
WORKDIR /workspace
# See the planner-stage note above.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
COPY --from=cargo-chef-builder /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/cargo-chef
RUN apt-get update \
    && apt-get install --yes --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook \
      --locked \
      --release \
      --features embedded-console-ui \
      --package ai-gateway \
      --recipe-path recipe.json

COPY . .
COPY --from=console-builder /workspace/web/console/dist ./web/console/dist
COPY --from=console-builder /workspace/web/console/node_modules ./web/console/node_modules
COPY --from=console-builder /workspace/web/console/production-dependencies.json ./web/console/production-dependencies.json
RUN cargo build \
      --locked \
      --release \
      --features embedded-console-ui \
      --package ai-gateway \
    && python3 scripts/generate-third-party-notices.py --output /out/licenses \
    && install -D -m 0755 target/release/ai-gateway /out/ai-gateway

FROM debian:bookworm-slim AS runtime
ARG VERSION=dev
ARG REVISION=unknown
ARG SOURCE_URL=unknown

LABEL org.opencontainers.image.title="ai-gateway" \
      org.opencontainers.image.description="OpenAI-compatible LLM request forwarding gateway" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="${SOURCE_URL}" \
      org.opencontainers.image.licenses="AGPL-3.0-only"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
      ca-certificates \
      curl \
      gosu \
      libgcc-s1 \
      tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 ai-gateway \
    && useradd \
      --uid 10001 \
      --gid ai-gateway \
      --home-dir /var/lib/ai-gateway \
      --shell /usr/sbin/nologin \
      ai-gateway \
    && install -d -m 0750 -o ai-gateway -g ai-gateway \
      /var/lib/ai-gateway/request-log-spool \
      /run/config

COPY --from=builder /out/ai-gateway /usr/local/bin/ai-gateway
COPY deploy/docker/entrypoint.sh /usr/local/bin/ai-gateway-entrypoint
COPY LICENSE /usr/share/doc/ai-gateway/LICENSE
COPY --from=builder /out/licenses/ /usr/share/doc/ai-gateway/

WORKDIR /var/lib/ai-gateway
EXPOSE 3000 3001
STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ai-gateway-entrypoint"]
CMD ["/run/ai-gateway/config.toml"]
