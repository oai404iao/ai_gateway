# AGENTS.md - ai-gateway

> Operational context for coding agents. Verify the current implementation before relying on the product blueprint in `docs/PRD.md`.

## What is ai-gateway?

`ai-gateway` is a single-binary Rust service intended to forward LLM requests in the OpenAI Chat Completions and Responses formats. It uses Axum/Tokio for HTTP, reqwest for upstream requests, PostgreSQL/SQLx for persistence, and `ArcSwap` for immutable runtime configuration snapshots. Rust 2024 with MSRV 1.85 is required (`Cargo.toml`).

This repository is currently a scaffold. The only public route is `GET /health` (204); proxying, persistence, transforms, routing, and workers have module boundaries but no implementation. Treat `docs/PRD.md` as the architectural source of truth for future work, not as a statement of implemented behavior.

## Repository Layout

```text
repo/
|-- src/
|   |-- main.rs                 # Process entry point: config load, tracing, TCP listener, Axum serve
|   |-- lib.rs                  # Module declarations for the single binary
|   |-- http/                   # Axum routes, middleware, and HTTP errors; currently /health only
|   |-- domain/                 # Domain entities; ApiFormat is the currently implemented type
|   |-- runtime_config/         # TOML deserialization and ArcSwap configuration snapshots
|   |-- observability/          # tracing-subscriber initialization
|   |-- application/            # Intended use cases (currently placeholder)
|   |-- routing/                # Intended model/channel selection (currently placeholder)
|   |-- transforms/             # Intended constrained transform DSL (currently placeholder)
|   |-- upstream/               # Intended reqwest clients/auth/header handling (currently placeholder)
|   |-- persistence/            # Intended SQLx repositories (currently placeholder)
|   `-- workers/                # Intended background jobs (currently placeholder)
|-- migrations/                 # SQLx migrations; currently empty except .gitkeep
|-- docs/PRD.md                 # Canonical product and architecture blueprint (Chinese)
|-- config.example.toml         # Canonical configuration template
|-- config.toml                 # Default config loaded by the binary
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

# Run using config.toml, or a supplied TOML path
cargo run
cargo run -- config.local.toml

# Start the development PostgreSQL service when persistence work needs it
docker compose up -d
```

There are no automated tests or CI workflows yet. Add focused tests with new behavior; use `cargo test` as the baseline verification. `docker-compose.yml` provides PostgreSQL only—the application is not containerized.

## Configuration Rules

- The binary loads the first CLI argument as TOML, defaulting to `config.toml` (`src/main.rs`). There is no dotenv support or automatic local-override merge.
- Keep `config.example.toml` and the deserialization types in `src/runtime_config/mod.rs` synchronized whenever configuration changes.
- `config.local.toml` is ignored and can be passed explicitly for local settings. Do not introduce `.env` configuration; `.gitignore` deliberately excludes it.
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
2. Add its documented default to `config.example.toml` and, if appropriate, the checked-in `config.toml`.
3. Wire the setting at its use site; preserve atomic snapshot replacement for runtime configuration.
4. Add tests for parsing/default behavior and run the standard Rust checks.

### Add an HTTP endpoint

1. Start in `src/http/mod.rs`; keep transport, middleware, and error mapping in `http`.
2. Put business orchestration in `application`, and domain rules/types in `domain`; do not embed persistence or upstream logic in handlers.
3. For proxy endpoints, add a format-specific entry that shares the intended proxy use case without mixing `ApiFormat` validation.
4. Add route and use-case tests, then run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test`.

### Add persistence

1. Add ordered SQL migration files in `migrations/`; do not edit database state manually as a substitute.
2. Keep database access behind `src/persistence/` repositories and domain/application boundaries.
3. SQLx compile-time query macros require a reachable schema or a prepared `.sqlx/` cache; that cache is intentionally ignored.
4. Start PostgreSQL with `docker compose up -d` and verify migrations and repository behavior.

## Gotchas

1. **The PRD is not the runtime.** Most modules named in its architecture section are placeholders. Confirm an API exists before integrating with it.
2. **`GET /health` is the only current endpoint.** Versioned proxy endpoints mentioned in the PRD are planned, not registered in the Axum router.
3. **`migrations/` is empty.** There is no schema or SQLx offline cache today; persistence changes must establish both deliberately.
4. **`config.local.toml` is not auto-loaded.** It is merely ignored by Git; invoke `cargo run -- config.local.toml` to use it.
5. **Configuration is TOML-only.** Avoid `.env` files and loading behavior that conflicts with the checked-in template.

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
| HTTP route registry | `src/http/mod.rs` |
| Startup and config-path behavior | `src/main.rs` |
| TOML config schema and snapshot API | `src/runtime_config/mod.rs` |
| Example configuration | `config.example.toml` |
| Local PostgreSQL service | `docker-compose.yml` |
