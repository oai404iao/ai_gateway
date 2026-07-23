# syntax=docker/dockerfile:1.7

ARG NODE_VERSION=24
ARG RUST_VERSION=1.85.0
ARG CARGO_CHEF_VERSION=0.1.71
ARG CARGO_CHEF_DIGEST=sha256:534c4d975e252b30309ca779af73d3a5932dbef19e40a5057980c14f3364984e

FROM node:${NODE_VERSION}-bookworm-slim AS console-builder
ARG PNPM_VERSION=11.7.0
ENV PNPM_HOME=/pnpm
ENV PATH="${PNPM_HOME}:${PATH}"
WORKDIR /workspace

RUN npm install --global "pnpm@${PNPM_VERSION}"
COPY web/console/package.json web/console/pnpm-lock.yaml web/console/pnpm-workspace.yaml ./web/console/
RUN --mount=type=cache,target=/pnpm/store \
    pnpm config set store-dir /pnpm/store \
    && pnpm --dir web/console install --frozen-lockfile
COPY web/console ./web/console
RUN pnpm --dir web/console build

FROM lukemathwalker/cargo-chef:${CARGO_CHEF_VERSION}-rust-${RUST_VERSION}-bookworm@${CARGO_CHEF_DIGEST} AS chef
WORKDIR /workspace

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook \
      --locked \
      --release \
      --features embedded-console-ui \
      --package ai-gateway \
      --recipe-path recipe.json

COPY . .
COPY --from=console-builder /workspace/web/console/dist ./web/console/dist
RUN cargo build \
      --locked \
      --release \
      --features embedded-console-ui \
      --package ai-gateway \
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
COPY LICENSES/OFL-1.1.txt /usr/share/doc/ai-gateway/OFL-1.1.txt
COPY web/console/NOTICES.md /usr/share/doc/ai-gateway/THIRD_PARTY_NOTICES.md

WORKDIR /var/lib/ai-gateway
EXPOSE 3000 3001
STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ai-gateway-entrypoint"]
CMD ["/run/ai-gateway/config.toml"]
