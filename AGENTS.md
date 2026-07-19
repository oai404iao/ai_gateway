# AGENTS.md - ai-gateway

> Operational context for coding agents. Verify the current implementation before relying on the product blueprint in `docs/PRD.md`.

## What is ai-gateway?

`ai-gateway` is a single-binary Rust service intended to forward LLM requests in the OpenAI Chat Completions and Responses formats. It uses Axum/Tokio for HTTP, reqwest for upstream requests, PostgreSQL/SQLx for persistence, and `ArcSwap` for immutable runtime configuration snapshots. Rust 2024 with MSRV 1.85 is required (`Cargo.toml`).

The implemented backend includes OpenAI-compatible Chat Completions and
Responses proxy routes, PostgreSQL-backed control-plane snapshots, a separate
JWT-authenticated Console API with `user`/`admin` roles, constrained
transforms, streaming/SSE forwarding, passive health, admission controls,
asynchronous request logs, and reusable upstream clients. Treat `docs/PRD.md`
as the architectural source of truth for future work, but verify current
behavior in code and the MVP task documents.

## Repository Layout

```text
repo/
|-- src/
|   |-- main.rs                 # Process entry point: config load, tracing, TCP listener, Axum serve
|   |-- lib.rs                  # Module declarations for the single binary
|   |-- http/                   # Public proxy routes and separate JWT-authenticated Console router
|   |-- admission/              # Process-local RPM, concurrency, and soft quota admission
|   |-- domain/                 # API formats, compiled routing, credentials, request-log events
|   |-- runtime_config/         # TOML deserialization and ArcSwap configuration snapshots
|   |-- observability/          # tracing-subscriber initialization
|   |-- application/            # Proxy, Console auth, control-plane publication, request-log sink
|   |-- routing/                # Priority/weight selection and passive health state
|   |-- transforms/             # Compiled constrained JSON/header/SSE transform DSL
|   |-- upstream/               # Reused reqwest clients, proxy policy, timeout resolution
|   |-- persistence/            # SQLx repositories, Console auth/session state, control-plane mutations, logs
|   `-- workers/                # Snapshot reload and async request-log persistence
|-- migrations/                 # PostgreSQL control-plane and log schema migrations
|-- tests/                      # Local, PostgreSQL, proxy, streaming, and opt-in real-upstream tests
|-- docs/PRD.md                 # Canonical product and architecture blueprint (Chinese)
|-- config.example.toml         # Canonical configuration template
|-- config.toml                 # Ignored current-directory runtime config; copy from example
|-- docker-compose.yml          # Development PostgreSQL service only
`-- Cargo.toml                  # Package metadata, MSRV, and dependency source of truth
```

## Build, Test, and Development

Run all commands from the repository root.

```bash
# Check, format, lint, and test
cargo check
cargo fmt --check
cargo clippy --all-targets
cargo test

# Run using ignored ./config.toml, or a supplied TOML path
cargo run
cargo run -- ./other-config.toml

# One-time first Console administrator; password is read only from stdin
cargo run -- bootstrap-admin --email admin@example.com --display-name "Initial Admin" --password-stdin < password.txt

# Start the development PostgreSQL service when persistence work needs it
docker compose up -d
```

The test suite contains unit tests and local/PostgreSQL integration tests.
`cargo test` is the baseline verification. The ignored
`tests/real_upstream/` contains paid external calls and must only run via
`./scripts/run-real-upstream-smoke.sh`; see `docs/real-upstream-smoke.md`.
**Any change to the forwarding path must also run this real-upstream script
before completion.** It serially verifies both `/v1/chat/completions` and
`/v1/responses`, with non-streaming and SSE requests. There is no CI workflow
yet. `docker-compose.yml` provides PostgreSQL only—the application is not
containerized.

## Configuration Rules

- The normal serve command loads the first CLI argument as TOML, defaulting to ignored `./config.toml` in the current working directory (`src/main.rs`). It does not use an XDG configuration directory. `bootstrap-admin` is a separate one-time CLI subcommand and requires `--password-stdin`. There is no dotenv support or automatic local-override merge.
- Keep `config.example.toml` and the deserialization types in `src/runtime_config/mod.rs` synchronized whenever configuration changes.
- `./config.toml` is ignored and loaded by default. A different current-directory TOML path can be passed explicitly. The binary never loads `.env` files. The sole exception is the ignored `.env.real-upstream` file, which `scripts/run-real-upstream-smoke.sh` may source for opt-in test credentials.
- Configuration changes intended for live reload should preserve the immutable-snapshot pattern: construct a complete `AppConfig`, then replace it atomically through `RuntimeConfig`.

## Architecture and Implementation Constraints

### Intended request flow

```text
Axum HTTP
  -> client API-key authentication
  -> API-format and model-rule resolution
  -> healthy channel selection (priority, then weight)
  -> request transformation and upstream authentication
  -> reqwest forwarding
  -> response pass-through or transformation
  -> asynchronous logging, usage, and billing
```

### Load-bearing rules from the PRD

- Support only `OpenAiChatCompletions` and `OpenAiResponses` (`src/domain/api_format.rs`). Keep their validation and routing paths separate: never fall back or transform between formats.
- `model_rules`, channel groups, and channels must agree on `api_format`; a model rule is unique by `(client_model, api_format)`.
- Parse a request only as far as necessary to obtain `model`; absent an enabled transform or model alias, forward the original request bytes without reserialization.
- Keep the fixed transform order: template defaults → channel overrides → upstream authentication. Configurable transforms must not alter protected or hop-by-hop headers.
- Stream upstream responses instead of buffering them. Do not retry or switch channels after sending response headers or any response byte to the client.
- Reuse reqwest clients keyed by proxy, TLS, and timeout policy. Do not create an HTTP client per request.
- Compile database-backed control-plane configuration into immutable runtime snapshots; the data plane must not query the database on every request.

## Common Change Workflows

### Add a configuration setting

1. Add the field to the appropriate TOML-deserialized type in `src/runtime_config/mod.rs`.
2. Add its documented default to the tracked `config.example.toml`; do not commit local `./config.toml`.
3. Wire the setting at its use site; preserve atomic snapshot replacement for runtime configuration.
4. Add tests for parsing/default behavior and run the standard Rust checks.

### Add an HTTP endpoint

1. Start in `src/http/mod.rs`; keep transport, middleware, and error mapping in `http`.
2. Put business orchestration in `application`, and domain rules/types in `domain`; do not embed persistence or upstream logic in handlers.
3. For proxy endpoints, add a format-specific entry that shares the intended proxy use case without mixing `ApiFormat` validation. For Console endpoints, use JWT middleware and enforce ownership in repository SQL rather than trusting body/path user IDs.
4. Add route and use-case tests, then run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test`.

### Add persistence

1. Add ordered SQL migration files in `migrations/`; do not edit database state manually as a substitute.
2. Keep database access behind `src/persistence/` repositories and domain/application boundaries.
3. SQLx compile-time query macros require a reachable schema or a prepared `.sqlx/` cache; that cache is intentionally ignored.
4. Start PostgreSQL with `docker compose up -d` and verify migrations and repository behavior.

### Change request forwarding

1. Add or update deterministic local and PostgreSQL tests as appropriate.
2. Run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test`.
3. Run `./scripts/run-real-upstream-smoke.sh` before considering the change complete. It requires the ignored `.env.real-upstream` file and makes paid calls to both configured upstream formats.
4. Do not print, commit, or copy credentials from `.env.real-upstream` into TOML, source, tests, or logs.

## Gotchas

1. **The PRD is not the runtime.** It includes later roadmap capabilities such as active health checks, cross-instance coordination, and generic retries. Confirm an API exists before integrating with it.
2. **Public and Console routes are separate.** `src/http/mod.rs` exposes the API-key data plane; `src/http/console.rs` is bound by `main` only when `[console].enabled` is set. Console is intended for an HTTPS reverse proxy, and `admin` is a role rather than a route namespace or static bearer token.
3. **Migrations are authoritative.** Add ordered SQL migrations for schema changes; there is no SQLx offline cache.
4. **`./config.toml` is auto-loaded.** It is intentionally ignored by Git; invoke `cargo run -- ./other-config.toml` only when using another file.
5. **Runtime configuration is TOML-only.** Do not add dotenv loading to the binary. Console Ed25519 keys are supplied by protected file paths, not TOML values. `.env.real-upstream` is test-script-only and must remain ignored.

## Code Style

- Use standard Rust formatting (`cargo fmt`) and linting (`cargo clippy`).
- Retain concise module-level `//!` documentation for module responsibilities.
- Model domain failures with typed errors (`thiserror` is available); propagate process-boundary failures from `main` as appropriate.
- Keep sensitive material in the intended secrecy/zeroization boundary. The PRD explicitly permits administrators and users to view stored client and upstream API keys; do not silently change that product behavior without a coordinated design change.

## Quick Reference

| Need | Source of truth |
|---|---|
| Package/MSRV/dependencies | `Cargo.toml` |
| Product architecture and constraints | `docs/PRD.md` |
| Supported client formats | `src/domain/api_format.rs` |
| HTTP route registry | `src/http/mod.rs` and `src/http/console.rs` |
| Startup and config-path behavior | `src/main.rs` |
| TOML config schema and snapshot API | `src/runtime_config/mod.rs` |
| Example configuration | `config.example.toml` |
| Console/JWT design and execution plan | `docs/console-auth-refactor-plan.md` |
| Current operational API documentation | `docs/mvp-usage.md` |
| Local PostgreSQL service | `docker-compose.yml` |
| Opt-in real upstream test | `docs/real-upstream-smoke.md` and `scripts/run-real-upstream-smoke.sh` |
