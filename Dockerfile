# syntax=docker/dockerfile:1.7

ARG NODE_VERSION=24
ARG RUST_VERSION=1.85.0

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

FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /workspace

COPY . .
COPY --from=console-builder /workspace/web/console/dist ./web/console/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    cargo build \
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
      org.opencontainers.image.source="${SOURCE_URL}"

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

WORKDIR /var/lib/ai-gateway
EXPOSE 3000 3001
STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/ai-gateway-entrypoint"]
CMD ["/run/ai-gateway/config.toml"]
