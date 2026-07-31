<p align="center">
  <img src="assets/logo.svg" width="96" height="96" alt="ai-gateway logo">
</p>

# ai-gateway

<p align="center">
  A production-oriented, single-binary Rust gateway for OpenAI-compatible LLM traffic.
</p>

<p align="center">
  <a href="https://github.com/oai404iao/ai_gateway/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/oai404iao/ai_gateway/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/oai404iao/ai_gateway/releases"><img alt="GitHub release" src="https://img.shields.io/github/v/release/oai404iao/ai_gateway"></a>
  <a href="rust-toolchain.toml"><img alt="Rust 1.92+" src="https://img.shields.io/badge/Rust-1.92%2B-orange"></a>
  <a href="LICENSE"><img alt="License: AGPL-3.0-only" src="https://img.shields.io/badge/license-AGPL--3.0--only-blue"></a>
</p>

<p align="center">
  <strong>English</strong>
  ·
  <a href="README.zh-CN.md">简体中文</a>
  ·
  <a href="docs/README.md">Documentation</a>
</p>

> **Status:** Current implementation, under active development. The public
> data plane supports Chat Completions, Responses, and non-streaming JSON
> Images generation.

`ai-gateway` is a self-hosted LLM request gateway built with Rust, Axum,
Tokio, SQLx, and PostgreSQL. It keeps routing on an immutable in-memory
snapshot, streams responses without whole-body buffering, and provides a
separate management Console for users and administrators.

## ✨ Features

- **OpenAI-compatible data plane** for Chat Completions, Responses, and Images
  generation over HTTP, SSE, and Responses WebSocket where applicable.
- **Priority and weighted routing** with passive health, optional session
  affinity, and controlled failover before upstream response headers arrive.
- **In-process upstream connectors** keep provider-specific authentication and
  request preparation inside the single Rust service. The first connector,
  Codex OAuth, adds subscription credentials, per-account proxies, token
  refresh, quota-aware draining, and shared provider-managed Responses
  HTTP/SSE/WebSocket plus Images generation channels.
- **Database-backed control plane** compiled into immutable runtime snapshots;
  proxy requests do not query PostgreSQL on the hot path.
- **Constrained transforms** for request JSON, headers, normal responses, and
  SSE events, with protected-header enforcement and upstream credential
  injection.
- **Admission and accounting** with process-local RPM/concurrency limits, soft
  USD quotas, durable request-log spooling, usage extraction, and asynchronous
  settlement.
- **Management Console** with JWT sessions, user/admin roles, API-key policy,
  routing and channel management, audit logs, and optimistic concurrency.
- **Single-binary deployment** with an optional embedded React Console UI; no
  Node.js process is required in production.

## 🔌 Supported APIs

| Endpoint | Authentication | Purpose |
| --- | --- | --- |
| `GET /health` | None | Liveness check; returns `204 No Content`. |
| `GET /v1/models` | Client API key | Lists models reachable by the key. |
| `POST /v1/chat/completions` | Client API key | Proxies Chat Completions requests. |
| `POST /v1/responses` | Client API key | Proxies Responses requests over HTTP or SSE. |
| `GET /v1/responses` + Upgrade | Client API key | Proxies sequential Responses requests over WebSocket. |
| `POST /v1/images/generations` | Client API key | Proxies non-streaming JSON Images generation requests. |

Each API format uses separate routing rules and never falls back or transforms
into another format. Images edits, multipart image bodies, image streaming,
embeddings, audio, files, batches, assistants, and fine-tuning APIs are
outside the current scope. See the
[OpenAI compatibility reference](docs/reference/openai-compatibility.md) for
the exact validation, streaming, retry, and pass-through boundaries.

## 🏗️ Architecture

```text
OpenAI-compatible client
  │  Bearer API key
  ▼
Public listener (/v1/*)
  → authentication and admission
  → immutable routing snapshot
  → channel selection and optional transforms
  → reusable HTTP client or pinned Responses WebSocket
  → streamed upstream response
  → durable asynchronous logging, usage, and settlement

Browser or Console client
  │  JWT through an HTTPS reverse proxy
  ▼
Console listener (/console/v1/*)
  → user/admin authorization
  → PostgreSQL transaction and audit record
  → immediate runtime snapshot publication
```

The public data-plane listener never serves the Console API or UI. The Console
listener is separate so it can have its own network and browser security
policy.

## 🚀 Quick start

### Prerequisites

- Rust **1.92** or newer; this repository pins Rust **1.97.1** for normal
  development and release builds.
- PostgreSQL, or Docker with Docker Compose.
- OpenSSL for generating the local password and Console signing keys.
- Node.js 24 and pnpm 11.17 only when developing or building the Console UI.

### 1. Start PostgreSQL and the gateway

```bash
git clone https://github.com/oai404iao/ai_gateway.git
cd ai_gateway

mkdir -p ./config
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password
cp config.example.toml ./config/config.toml

docker compose up -d
cargo run
```

Verify the public listener:

```bash
curl -i http://127.0.0.1:3000/health
# HTTP/1.1 204 No Content
```

The binary applies database migrations automatically. The default template
starts with an empty control plane and leaves the Console disabled, so health
checks work immediately but proxy requests require the next steps.

The default configuration path is `./config/config.toml`. A different path can
be passed as the first argument:

```bash
cargo run -- ./config/other-config.toml
```

The service does not load `.env` files or use an XDG configuration directory.

### 2. Enable the Console

Generate an Ed25519 key pair:

```bash
openssl genpkey -algorithm Ed25519 \
  -out ./config/console-jwt-private.pem
openssl pkey \
  -in ./config/console-jwt-private.pem \
  -pubout \
  -out ./config/console-jwt-public.pem
chmod 600 ./config/console-jwt-private.pem
```

Uncomment and review `[console]` and `[auth]` in
[`config.example.toml`](config.example.toml), then apply those settings to
`./config/config.toml`. The Console listener should remain behind an HTTPS
reverse proxy; the gateway does not terminate TLS.

Create the first administrator by reading the password from standard input:

```bash
cargo run -- bootstrap-admin \
  --email admin@example.com \
  --display-name "Initial Admin" \
  --password-stdin < /secure/path/admin-password.txt
```

Restart the gateway after enabling the Console. By default, the public API is
available at `http://127.0.0.1:3000` and the Console API at
`http://127.0.0.1:3001`.

### 3. Provision a route

Use the Console UI or API to create:

1. A priced model.
2. A channel group for the required API format and connector. Creating a Codex
   OAuth Responses group also creates a disabled Images group backed by the
   same credential pool.
3. A channel with its upstream URL, credentials, and available models.
4. A model rule mapping the client model to the upstream model and route.
5. A client API key with `proxy` permission; add `models.read` for
   `/v1/models`.

Create separate model rules for Chat Completions, Responses, and Images, even
when they use the same provider. The
[operations guide](docs/user/operations.md) documents the full Console route
inventory and control-plane behavior.

### 4. Send a request

```bash
export AI_GATEWAY_API_KEY='replace-with-client-key'

curl http://127.0.0.1:3000/v1/chat/completions \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "gateway-chat-model",
    "messages": [
      {"role": "user", "content": "Say hello."}
    ]
  }'
```

OpenAI-compatible clients can use the public listener as their base URL. A
Responses route uses the same client authentication but sends requests to
`/v1/responses`.

## 🖥️ Console UI

For frontend development, run the Rust API and Vite dev server separately:

```bash
# Terminal 1
cargo run

# Terminal 2
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console dev
```

Then open `https://console.localhost:5173`. The Vite server proxies
`/console/v1/*` to the gateway.

For a production single binary, build and embed the SPA:

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build
cargo build --release --features embedded-console-ui
```

Set `[console].ui_enabled = true` before running that binary. UI assets are
served only from the Console listener. See
[`web/console/README.md`](web/console/README.md) for frontend workflows.

## ⚙️ Configuration

Configuration is split into two layers:

| Layer | Source | Examples |
| --- | --- | --- |
| Process/bootstrap | TOML | Listeners, PostgreSQL, request limits, default timeouts, durable spool, Console JWT key paths. |
| Dynamic control plane | PostgreSQL through the Console | Users, API keys, models, routes, channels, proxies, transforms, and forwarding settings. |

Console writes validate the complete candidate configuration before commit and
publish a new immutable snapshot immediately afterward. A periodic reload
worker provides cross-process convergence.

Start with [`config.example.toml`](config.example.toml). Production sizing,
PostgreSQL tuning, storage, and operational metrics are covered in the
[production configuration guide](docs/user/production-configuration.md).

## 🐳 Production deployment

Tagged releases publish:

- a multi-platform image at `ghcr.io/oai404iao/ai_gateway`;
- GitHub Release archives containing the production binary and required
  license material.

Pin an immutable version in production instead of relying on `latest`.
[`docker-compose.prd.yaml`](docker-compose.prd.yaml) provides a complete
single-host Gateway and PostgreSQL stack with persistent database and
request-log spool volumes.

This Compose setup is a deployment baseline, not a high-availability platform:
TLS termination, backups, PITR, monitoring, alerting, and PostgreSQL HA remain
operator responsibilities. Follow the
[production deployment guide](docs/user/production-deployment.md) before
starting the stack.

## 🧭 Runtime boundaries

- Requests are authenticated and admitted before their body is read.
- Original request bytes are preserved unless a model alias or configured body
  transform requires reserialization.
- Upstream responses are streamed; the gateway does not buffer the complete
  response for normal forwarding or usage collection.
- Automatic failover is limited to connection failures and timeouts before
  upstream response headers. It never switches channels after headers or
  response bytes are sent.
- RPM, concurrency, passive health, session affinity, and WebSocket pools are
  process-local rather than cluster-coordinated.
- Request logs do not persist prompts, completions, full headers, API keys,
  cookies, or unredacted upstream error bodies.
- All balances, quotas, prices, and request costs use USD.

## 🛠️ Development

Start PostgreSQL, then run the Rust checks from the repository root:

```bash
docker compose up -d
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

Console checks:

```bash
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console generate:api:check
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build
```

The Console OpenAPI contract is
[`docs/openapi/console-v1.yaml`](docs/openapi/console-v1.yaml); generated
TypeScript declarations must not be edited by hand. Forwarding-path changes
also require the explicitly authorized real-upstream smoke test described in
[`docs/development/real-upstream-smoke.md`](docs/development/real-upstream-smoke.md).

## 🗂️ Repository layout

```text
src/                    Rust service
  application/          Proxy and Console orchestration
  domain/               API formats, routing, and value objects
  http/                 Public and Console Axum routers
  persistence/          PostgreSQL repositories
  routing/              Selection and passive health
  transforms/           Constrained transform engine
  upstream/             Reusable HTTP and WebSocket clients
  workers/              Reload, logging, projection, and settlement
web/console/            React + TypeScript management SPA
migrations/             Ordered PostgreSQL migrations
tests/                  Rust integration tests
tools/forwarding-perf/  Opt-in forwarding benchmark tooling
docs/                   User, development, and compatibility documentation
```

## 📚 Documentation

| Guide | Audience |
| --- | --- |
| [Documentation center](docs/README.md) | Complete document map and source precedence. |
| [Operations](docs/user/operations.md) | Data plane, Console API, logging, billing, and runtime behavior. |
| [Production configuration](docs/user/production-configuration.md) | Capacity, PostgreSQL, storage, and observability. |
| [Production deployment](docs/user/production-deployment.md) | Compose, secrets, upgrades, and deployment boundaries. |
| [OpenAI compatibility](docs/reference/openai-compatibility.md) | Supported semantics and intentional differences. |
| [Current architecture](docs/development/architecture.md) | Runtime design and module boundaries. |
| [Release process](docs/development/releasing.md) | Versioning, verification, packaging, and publication. |

## 🔐 Security

- Keep `./config/config.toml`, database passwords, upstream credentials, client
  API keys, and Console JWT private keys out of version control.
- Prefer `[database].password_file` over embedding a password in TOML.
- Treat PostgreSQL, backups, and Console access as credential-sensitive.
- Expose the Console only through a deliberately configured HTTPS reverse
  proxy and restrict the public listener to the intended network boundary.

## 📜 License

`ai-gateway` is licensed under the
[GNU Affero General Public License v3.0 only](LICENSE)
(`AGPL-3.0-only`).

If you modify the program and make the modified version available to users
over a network, AGPL section 13 requires offering those users access to the
corresponding source code. Third-party components retain their own licenses;
see [`LICENSES/`](LICENSES/) and
[`web/console/NOTICES.md`](web/console/NOTICES.md).
