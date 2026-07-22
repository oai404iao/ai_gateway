# ai-gateway

[中文文档](README.zh-CN.md) | English

`ai-gateway` is a single-binary Rust gateway for forwarding OpenAI-compatible LLM requests. It exposes the Chat Completions and Responses APIs to clients, routes each request through a PostgreSQL-backed control plane, and forwards it to a configured upstream provider.

The public data plane and the management Console API are intentionally separate listeners:

- **Data plane** (`/v1/*`): client API keys and OpenAI-compatible requests.
- **Console API** (`/console/v1/*`): JWT-authenticated user self-service and administrator control-plane management.

## Highlights

- Supports **only** OpenAI Chat Completions and Responses; the two formats never fall back to one another.
- Routes by `(client model, API format)` using channel-group priority and weighted channel selection.
- Compiles PostgreSQL control-plane records into immutable in-memory snapshots, so proxy requests do not query the database.
- Rewrites model aliases and applies constrained JSON, header, response, and SSE transforms when configured.
- Removes client credentials and hop-by-hop headers before injecting channel-specific upstream authentication.
- Streams upstream responses without buffering the whole response; it never retries or changes channels after response headers or bytes are sent.
- Provides process-local RPM, concurrency, and soft quota admission controls, passive connection health, asynchronous request logs, usage extraction, and USD-only settlement.
- Includes a separate JWT Console API with invitations, rotating refresh sessions, user/admin roles, audit logs, and optimistic concurrency for most mutable resources.

## Architecture

```text
OpenAI-compatible client
  │ Bearer API key
  ▼
Public listener (/v1/*)
  → authentication and admission
  → immutable routing snapshot
  → channel selection and optional transforms
  → reusable reqwest upstream client
  → streaming upstream response
  → asynchronous request logging / usage / settlement

Console client
  │ JWT
  ▼
Separate Console listener (/console/v1/*)
  → user or admin authorization
  → PostgreSQL control-plane transaction + audit record
  → immediate runtime snapshot publication
```

## Requirements

- Rust **1.85** or newer (Rust 2024 edition)
- PostgreSQL
- Docker Compose is optional: `docker-compose.yml` provides PostgreSQL for
  development, while `docker-compose.prd.yaml` can run the complete production
  stack from a pulled or locally built Gateway image
- OpenSSL is useful for generating the local database password and Console
  Ed25519 keys

## Quick start / 快速启动

> `./config/config.toml` is ignored by Git and is the default configuration
> location. Local JWT files also belong in `./config/`.
> The service does not load `.env` files or use an XDG configuration directory.
> 中文启动说明见下方对应步骤及 [中文文档](README.zh-CN.md)。

### 1. Create local secrets/configuration and start PostgreSQL

```bash
mkdir -p ./config
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password
cp config.example.toml ./config/config.toml
docker compose up -d
```

Edit `./config/config.toml` as needed. At minimum, verify `[database]`, public `[server]`, and `[upstream]` timeout settings. The supplied Docker Compose service matches the example database URL.
The example reads the database password from
`./config/postgres-password`; it does not embed a default password in TOML or
Compose. The Compose defaults target a 4–8 GiB single-node host and can be
overridden with documented `AI_GATEWAY_POSTGRES_*` variables.

If you are upgrading from the former root-level layout, move local files once:

```bash
mkdir -p ./config
mv ./config.toml ./config/config.toml
mv ./console-jwt-private.pem ./console-jwt-public.pem ./config/
```

The binary applies database migrations automatically at startup.

### 2. Enable the Console API (recommended)

The Console API is the supported way to manage users, API keys, models, routes, channels, and transforms. Generate an Ed25519 key pair in `./config/`; these files are ignored by Git:

```bash
openssl genpkey -algorithm Ed25519 \
  -out ./config/console-jwt-private.pem
openssl pkey \
  -in ./config/console-jwt-private.pem \
  -pubout \
  -out ./config/console-jwt-public.pem
chmod 600 ./config/console-jwt-private.pem
```

Then enable `[console]` and fill in `[auth]` in `./config/config.toml` using the commented template in `config.example.toml`. Use `./config/console-jwt-private.pem` and `./config/console-jwt-public.pem` for the two generated PEM paths.

The service does **not** terminate TLS. Put the Console listener behind a correctly configured HTTPS reverse proxy before exposing it to browsers or the Internet.

### 3. Create the first administrator

The one-time bootstrap command succeeds only when no active administrator exists. Read the password from a protected file or secret manager through standard input:

```bash
cargo run -- bootstrap-admin \
  --email admin@example.com \
  --display-name "Initial Admin" \
  --password-stdin < /secure/path/admin-password.txt
```

### 4. Run the gateway

```bash
cargo run
```

Verify the public listener:

```bash
curl -i http://127.0.0.1:3000/health
# HTTP/1.1 204 No Content
```

An empty control plane can start successfully, but it cannot proxy requests until you provision a user/API key and routing configuration.

### 5. Provision the control plane

Log in to the Console API with the bootstrap administrator, then use its access JWT for subsequent Console requests. If you enabled the Console web UI (`[console].ui_enabled = true` with the `embedded-console-ui` feature, or the Vite dev server in development), browse it instead and skip the curl below — the UI drives the same Console API.

```bash
curl --request POST http://127.0.0.1:3001/console/v1/auth/login \
  --header 'Content-Type: application/json' \
  --data '{"email":"admin@example.com","password":"your-password"}'
```

For a working data-plane route, create compatible records in this order:

1. A priced **model**.
2. A **channel group** for the required API format.
3. A **channel** in that group with its upstream URL, upstream credentials, and supported upstream model name.
4. A **model rule** that maps the client model name to the priced model, upstream model name, and route target.
5. A client **API key** with `proxy` permission; add `models.read` if it must call `/v1/models`.

A Chat Completions route and a Responses route are separate configurations, even when they use the same upstream provider or model name. Use the Console API rather than editing control-plane tables directly.

See [the operational API guide](docs/mvp-usage.md) for Console route coverage and behavior.

## Production Docker deployment

The release image contains the Rust binary and embedded Console UI. The
full-stack production Compose keeps PostgreSQL private to the Compose network,
binds the public and Console listeners to host loopback by default, and stores
the request-log spool in a dedicated persistent volume.

Prepare the ignored configuration and secrets:

```bash
mkdir -p ./config
cp deploy/compose/config.example.toml ./config/config.prd.toml
cp deploy/compose/env.example ./config/compose.prd.env
# Generate ./config/postgres-password and the two Console JWT PEM files.
```

Then either pull the pinned release image or build it from the current
checkout:

```bash
docker compose --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml pull gateway
# Or: docker compose --env-file ./config/compose.prd.env \
#       -f docker-compose.prd.yaml build gateway

docker compose --env-file ./config/compose.prd.env \
  -f docker-compose.prd.yaml up -d --no-build
```

See [the production Docker deployment guide](docs/production-deployment.md)
for key generation, bootstrap-admin, reverse-proxy/TLS requirements, upgrades,
and backup boundaries.

## Manual forwarding performance harness

The repository includes an explicitly invoked, isolated end-to-end forwarding
performance harness. It creates a throwaway PostgreSQL database, starts a Mock
LLM upstream and a fresh release gateway process, runs direct and proxied JSON
and SSE loads, verifies asynchronous request-log persistence, and writes
Markdown/JSON reports.

It is never run by ordinary `cargo test` or CI commands:

```bash
docker compose up -d
./scripts/run-forwarding-perf.sh --profile quick
```

See [docs/forwarding-performance.md](docs/forwarding-performance.md) for the
design, scenarios, safety model, and `standard` profile.

## Using the data plane

All public API endpoints use `Authorization: Bearer <client-api-key>`.

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | Unauthenticated liveness endpoint; returns `204`. |
| `GET /v1/models` | Lists models reachable by the API key; requires both `proxy` and `models.read` for at least one format. |
| `POST /v1/chat/completions` | Proxies a Chat Completions request only. |
| `POST /v1/responses` | Proxies a Responses request only. |

After provisioning a matching rule and API key:

```bash
export GATEWAY_URL=http://127.0.0.1:3000
export AI_GATEWAY_API_KEY='replace-with-client-key'

curl "$GATEWAY_URL/v1/models" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY"

curl --request POST "$GATEWAY_URL/v1/chat/completions" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "gateway-chat-model",
    "messages": [{"role": "user", "content": "Say hello."}]
  }'
```

For Responses, configure a separate `open_ai_responses` model rule and send a normal Responses request:

```bash
curl --request POST "$GATEWAY_URL/v1/responses" \
  --header "Authorization: Bearer $AI_GATEWAY_API_KEY" \
  --header 'Content-Type: application/json' \
  --data '{
    "model": "gateway-responses-model",
    "input": "Say hello."
  }'
```

The gateway forwards upstream status codes and response bodies. It preserves streaming behavior; use the corresponding OpenAI request streaming fields when your client needs SSE.

## Configuration model

TOML is for process/bootstrap settings only. By default the binary reads
`./config/config.toml`; the ignored `./config/` directory also holds local JWT
key files.

| Area | Examples |
| --- | --- |
| `[server]` | Public listener and graceful-shutdown deadline. |
| `[request_limits]` | Independent proxy, Console, and authentication body-size limits. |
| `[database]` | PostgreSQL URL, pool size, and connection timeout. |
| `[upstream]` | Default connect, response-header, and stream-idle timeouts. |
| `[runtime_config]` | Periodic PostgreSQL control-plane reload interval. |
| `[passive_health]` | Connection-failure threshold and cooldown. |
| `[request_logging]` | Durable local spool, isolated DB pool, COPY ingress, projection, settlement, and telemetry limits. |
| `[console]` and `[auth]` | Optional dedicated Console listener and JWT key-file settings. |

Dynamic data-plane configuration—users, API keys, models, model rules, channel groups, channels, proxies, and transform templates—lives in PostgreSQL and is compiled into an immutable runtime snapshot. Dynamic `[[api_keys]]`, `[[channels]]`, and `[[model_rules]]` TOML tables are deliberately unsupported.

Configuration writes through the Console API validate the complete candidate snapshot and publish it immediately after commit. A periodic reloader also refreshes the snapshot from PostgreSQL.

Terminal request logs first cross a local recoverable spool, then enter a
low-index PostgreSQL staging table through `COPY FROM`, and are projected and
settled asynchronously. See
[docs/request-log-durability.md](docs/request-log-durability.md) for guarantees,
failure boundaries, and operational metrics.

The production sizing assumptions, PostgreSQL settings, password-file setup,
storage guidance, and small/large machine profiles are documented in
[docs/production-configuration.md](docs/production-configuration.md).

## Console API

The Console listener is separate from the public listener and uses short-lived JWT access tokens. Successful login, refresh, and invitation activation also issue a rotating `HttpOnly; Secure; SameSite=Lax` refresh cookie.

- Self-service endpoints are under `/console/v1/me` and derive ownership from the JWT subject.
- Administrator-only control-plane endpoints manage users, API-key policies, API keys, models, routes, network proxies, transform templates, request logs, audit logs, and reloads.
- Most mutable resources use `ETag` on `GET` and require `If-Match` on `PUT` for optimistic concurrency.
- `admin` is a user role—not a separate `/admin` API namespace or a process-wide static token.

Refer to [docs/mvp-usage.md](docs/mvp-usage.md) for the route inventory and [docs/console-auth-refactor-plan.md](docs/console-auth-refactor-plan.md) for the Console authentication design.

## Console web UI

The Console UI is a React + TypeScript + Vite + Tailwind CSS + shadcn/ui
(Radix) SPA that lives under `web/console/`. Release builds can embed the
Vite assets in the Rust binary and serve them only from the dedicated Console
listener at the same origin as the existing API:

```text
https://console.example.com/                 # SPA
https://console.example.com/assets/*          # static assets
https://console.example.com/console/v1/*      # existing Console API
```

It never exposes UI resources on the public `/v1/*` listener and does not
introduce SSR or a long-running Node service. Access tokens stay in memory;
the rotating refresh cookie keeps its `HttpOnly; Secure; SameSite=Lax`
attributes.

The UI ships in two run modes. Prerequisites for both: PostgreSQL is up,
`./config/config.toml` has `[console]` enabled with an `[auth]` JWT key pair
(see Quick start steps 1–2), and the bootstrap administrator exists (step 3).

### Run mode A — development (live reload, no embedding)

Run the gateway (Console API only) and the Vite dev server separately. The dev
server serves the SPA over HTTPS and proxies `/console/v1/*` to the gateway's
Console listener, so you edit frontend code with hot reload while the Rust
binary answers real API calls.

```bash
# Terminal 1 — gateway on 127.0.0.1:3000 (public) and 127.0.0.1:3001 (Console)
cargo run

# Terminal 2 — Vite dev server on https://console.localhost:5173
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console dev
```

Browse `https://console.localhost:5173`. The `console.localhost` host and
self-signed HTTPS make the `__Host-` refresh cookie and its `Secure` attribute
behave like production. `ui_enabled` is irrelevant in this mode (the gateway
serves only the API); the Vite dev server is a Node process for development,
not a production runtime.

### Run mode B — production (embedded single binary)

Build the frontend, then build the gateway with the `embedded-console-ui`
feature so the Vite assets are baked into the binary and served only from the
Console listener. No Node process runs in production.

```bash
# 1. Build the frontend (produces web/console/dist)
pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console build

# 2. Build the gateway with the embedded UI feature
cargo build --release --features embedded-console-ui
```

Set `[console].ui_enabled = true` in `./config/config.toml` to mount the SPA on
the Console listener. With `ui_enabled = true` but the feature compiled out,
startup is rejected. (In debug builds, `rust-embed` reads `web/console/dist`
from disk at runtime, so the dist must still exist; release builds embed it.)

```bash
# Run the single binary behind an HTTPS reverse proxy
cargo run --release --features embedded-console-ui
# Browse the Console listener origin, e.g. https://console.example.com/
```

### Frontend checks and tests

```bash
pnpm --dir web/console typecheck   # tsc --noEmit (strict)
pnpm --dir web/console lint         # oxlint
pnpm --dir web/console test         # vitest component tests (jsdom + MSW)
pnpm --dir web/console e2e          # Playwright browser tests (installs Chromium)
pnpm --dir web/console generate:api:check   # OpenAPI spec/type drift gate
```

See `web/console/README.md` for the full command list, layout, and the
OpenAPI contract workflow, and the [Console Web UI design and implementation
plan](docs/console-ui-design.md) for the repository layout, auth/caching model,
shadcn conventions, and phased delivery plan.

## Runtime behavior and boundaries

- Requests are authenticated and admitted before the body is read.
- Model aliases and transforms cause only the necessary request reserialization; otherwise the original request bytes are forwarded.
- Transform order is fixed: template defaults → channel overrides → protected-header cleanup → upstream authentication.
- Client `Authorization`, hop-by-hop headers, and `Connection`-declared headers are never forwarded upstream.
- Passive health reacts to pre-header connection failures. There is no active health-check worker.
- RPM, concurrency, and soft quota admission are process-local; there is no cross-instance coordination.
- Usage and billing are asynchronous and best effort. Quotas are soft prechecks based on settled usage; they do not reserve a cost before forwarding.
- Request logging does not persist prompts, completions, full headers, API keys, cookies, or unredacted upstream error content.

All balances, quotas, model prices, request costs, and statistics use USD. Current scope does not include embeddings, images, audio, files, batches, assistants, fine-tuning, generic automatic retries, TLS termination, a financial ledger, refunds/top-ups, or currency conversion.

## Development and verification

Run these commands from the repository root:

```bash
cargo check
cargo fmt --check
cargo clippy --all-targets
cargo test
```

Release preparation and tag publication are documented in
[`docs/releasing.md`](docs/releasing.md). The local release gate is:

```bash
./scripts/verify-release.sh 0.1.0
```

Changes to the request-forwarding path also require the opt-in paid real-upstream smoke test. Use a dedicated low-spend credential and keep it only in the ignored local secrets file:

```bash
cp .env.real-upstream.example .env.real-upstream
# Fill local values, then:
./scripts/run-real-upstream-smoke.sh
```

This script is the sole `.env` exception in the repository. See [docs/real-upstream-smoke.md](docs/real-upstream-smoke.md) before running it.

## Repository layout

```text
src/
  application/       Proxy, Console auth, control-plane, logging, usage
  admission/         Process-local RPM, concurrency, and soft quota controls
  domain/            API formats, routing, credentials, request-log types
  http/              Public Axum routes and separate Console router
  persistence/       SQLx repositories and migrations integration
  runtime_config/    TOML bootstrap config and ArcSwap snapshots
  routing/           Priority/weight selection and passive health
  transforms/        Constrained JSON, header, response, and SSE transforms
  upstream/          Reusable reqwest clients and proxy/timeout policies
  workers/           Snapshot reload and asynchronous request-log workers
migrations/          PostgreSQL schema migrations
deploy/postgres/     Compose initialization helpers
docs/                Product, operational, and design documentation
config/              Ignored runtime configuration, DB password, and JWT keys
config.example.toml  Tracked configuration template
tests/               Local and PostgreSQL integration tests
```

## Security notes

- Keep JWT private keys in ignored local files (the recommended development
  location is `./config/console-jwt-private.pem`) with restrictive filesystem
  permissions.
- Keep `./config/postgres-password` mode `0600`; prefer
  `[database].password_file` over embedding a database password in TOML.
- Treat the database, backups, and Console access as credential-sensitive: control-plane records include client and upstream credentials.
- Do not place client/upstream credentials or JWT private-key material in TOML, source files, logs, test fixtures, or shell history.
- Expose the Console API only through HTTPS with a deliberate origin policy. Keep the public data-plane listener appropriately network-restricted as well.

## Documentation

- [Operational usage and endpoint guide](docs/mvp-usage.md)
- [Production configuration and capacity tuning](docs/production-configuration.md)
- [Production Docker deployment](docs/production-deployment.md)
- [Version release process](docs/releasing.md)
- [Console Web UI design and implementation plan](docs/console-ui-design.md)
- [Database and control-plane design](docs/database-design.md)
- [Real-upstream smoke-test guide](docs/real-upstream-smoke.md)
- [Product requirements document (Chinese)](docs/PRD.md)

## License

This project is marked `UNLICENSED` in `Cargo.toml`. Consult the repository owner before redistributing or using it outside its intended environment.
