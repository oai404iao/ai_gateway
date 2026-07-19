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
- Provides process-local RPM, concurrency, and soft quota admission controls, passive connection health, asynchronous request logs, usage extraction, and settlement.
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
- Docker Compose is optional but provided for local PostgreSQL development
- OpenSSL is useful when enabling the Console API and generating local Ed25519 keys

## Quick start

> `config.local.toml` is ignored by Git. The service does not load `.env` files.

### 1. Start PostgreSQL and create a local configuration

```bash
docker compose up -d
cp config.example.toml config.local.toml
```

Edit `config.local.toml` as needed. At minimum, verify `[database]`, public `[server]`, and `[upstream]` timeout settings. The supplied Docker Compose service matches the example database URL.

The binary applies database migrations automatically at startup.

### 2. Enable the Console API (recommended)

The Console API is the supported way to manage users, API keys, models, routes, channels, and transforms. Generate an Ed25519 key pair in a protected directory outside the repository, then use **absolute paths** in `config.local.toml`:

```bash
install -d -m 700 "$HOME/.config/ai-gateway"
openssl genpkey -algorithm Ed25519 \
  -out "$HOME/.config/ai-gateway/console-jwt-private.pem"
openssl pkey \
  -in "$HOME/.config/ai-gateway/console-jwt-private.pem" \
  -pubout \
  -out "$HOME/.config/ai-gateway/console-jwt-public.pem"
chmod 600 "$HOME/.config/ai-gateway/console-jwt-private.pem"
```

Then enable `[console]` and fill in `[auth]` using the commented template in `config.example.toml`. For example, configure a dedicated listener on `127.0.0.1:3001`, an explicit browser-origin allowlist, and the two generated PEM paths.

The service does **not** terminate TLS. Put the Console listener behind a correctly configured HTTPS reverse proxy before exposing it to browsers or the Internet.

### 3. Create the first administrator

The one-time bootstrap command succeeds only when no active administrator exists. Read the password from a protected file or secret manager through standard input:

```bash
cargo run -- bootstrap-admin \
  --config config.local.toml \
  --email admin@example.com \
  --display-name "Initial Admin" \
  --password-stdin < /secure/path/admin-password.txt
```

### 4. Run the gateway

```bash
cargo run -- config.local.toml
```

Verify the public listener:

```bash
curl -i http://127.0.0.1:3000/health
# HTTP/1.1 204 No Content
```

An empty control plane can start successfully, but it cannot proxy requests until you provision a user/API key and routing configuration.

### 5. Provision the control plane

Log in to the Console API with the bootstrap administrator, then use its access JWT for subsequent Console requests:

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

TOML is for process/bootstrap settings only:

| Area | Examples |
| --- | --- |
| `[server]` | Public listener and graceful-shutdown deadline. |
| `[request_limits]` | Independent proxy, Console, and authentication body-size limits. |
| `[database]` | PostgreSQL URL, pool size, and connection timeout. |
| `[upstream]` | Default connect, response-header, and stream-idle timeouts. |
| `[runtime_config]` | Periodic PostgreSQL control-plane reload interval. |
| `[passive_health]` | Connection-failure threshold and cooldown. |
| `[request_logging]` | Bounded asynchronous log queue capacity. |
| `[console]` and `[auth]` | Optional dedicated Console listener and JWT key-file settings. |

Dynamic data-plane configuration—users, API keys, models, model rules, channel groups, channels, proxies, and transform templates—lives in PostgreSQL and is compiled into an immutable runtime snapshot. Dynamic `[[api_keys]]`, `[[channels]]`, and `[[model_rules]]` TOML tables are deliberately unsupported.

Configuration writes through the Console API validate the complete candidate snapshot and publish it immediately after commit. A periodic reloader also refreshes the snapshot from PostgreSQL.

## Console API

The Console listener is separate from the public listener and uses short-lived JWT access tokens. Successful login, refresh, and invitation activation also issue a rotating `HttpOnly; Secure; SameSite=Lax` refresh cookie.

- Self-service endpoints are under `/console/v1/me` and derive ownership from the JWT subject.
- Administrator-only control-plane endpoints manage users, API-key policies, API keys, models, routes, network proxies, transform templates, request logs, audit logs, and reloads.
- Most mutable resources use `ETag` on `GET` and require `If-Match` on `PUT` for optimistic concurrency.
- `admin` is a user role—not a separate `/admin` API namespace or a process-wide static token.

Refer to [docs/mvp-usage.md](docs/mvp-usage.md) for the route inventory and [docs/console-auth-refactor-plan.md](docs/console-auth-refactor-plan.md) for the Console authentication design.

## Runtime behavior and boundaries

- Requests are authenticated and admitted before the body is read.
- Model aliases and transforms cause only the necessary request reserialization; otherwise the original request bytes are forwarded.
- Transform order is fixed: template defaults → channel overrides → protected-header cleanup → upstream authentication.
- Client `Authorization`, hop-by-hop headers, and `Connection`-declared headers are never forwarded upstream.
- Passive health reacts to pre-header connection failures. There is no active health-check worker.
- RPM, concurrency, and soft quota admission are process-local; there is no cross-instance coordination.
- Usage and billing are asynchronous and best effort. Quotas are soft prechecks based on settled usage; they do not reserve a cost before forwarding.
- Request logging does not persist prompts, completions, full headers, API keys, cookies, or unredacted upstream error content.

Current scope does not include embeddings, images, audio, files, batches, assistants, fine-tuning, generic automatic retries, TLS termination, a financial ledger, refunds/top-ups, or multi-currency conversion.

## Development and verification

Run these commands from the repository root:

```bash
cargo check
cargo fmt --check
cargo clippy --all-targets
cargo test
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
docs/                Product, operational, and design documentation
tests/               Local and PostgreSQL integration tests
```

## Security notes

- Keep JWT private keys outside the repository with restrictive filesystem permissions.
- Treat the database, backups, and Console access as credential-sensitive: control-plane records include client and upstream credentials.
- Do not place client/upstream credentials or JWT private-key material in TOML, source files, logs, test fixtures, or shell history.
- Expose the Console API only through HTTPS with a deliberate origin policy. Keep the public data-plane listener appropriately network-restricted as well.

## Documentation

- [Operational usage and endpoint guide](docs/mvp-usage.md)
- [Database and control-plane design](docs/database-design.md)
- [Real-upstream smoke-test guide](docs/real-upstream-smoke.md)
- [Product requirements document (Chinese)](docs/PRD.md)

## License

This project is marked `UNLICENSED` in `Cargo.toml`. Consult the repository owner before redistributing or using it outside its intended environment.
