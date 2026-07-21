# AGENTS.md - ai-gateway

> Operational context for coding agents. Verify the current implementation before relying on the product blueprint in `docs/PRD.md`.

## What is ai-gateway?

`ai-gateway` is a single-binary Rust production service intended to forward LLM requests in the OpenAI Chat Completions and Responses formats. It uses Axum/Tokio for HTTP, reqwest for upstream requests, PostgreSQL/SQLx for persistence, and `ArcSwap` for immutable runtime configuration snapshots. Rust 2024 with MSRV 1.85 is required (`Cargo.toml`). The Cargo workspace also contains the development-only `ai-gateway-perf` package under `tools/forwarding-perf/`; it is never linked into the production binary.

The implemented backend includes OpenAI-compatible Chat Completions and
Responses proxy routes, PostgreSQL-backed control-plane snapshots, a separate
JWT-authenticated Console API with `user`/`admin` roles, constrained
transforms, streaming/SSE forwarding, passive health, admission controls,
durable spooled request logs, and reusable upstream clients. A React + TypeScript
Console web UI lives under `web/console/` and can be embedded into the binary
as static assets via the optional `embedded-console-ui` cargo feature, served
only from the Console listener. Treat `docs/PRD.md` as the architectural
source of truth for future work, but verify current behavior in code and the
MVP task documents.

The Console API contract is an authoritative OpenAPI spec at
`docs/openapi/console-v1.yaml`; the frontend's TypeScript types are generated
from it (never hand-edited). There is no SSR or long-running Node service in
production — Node is build/test/dev tooling only.

## Repository Layout

```text
repo/
|-- src/
|   |-- main.rs                 # Process entry point: config load, tracing, TCP listeners, Axum serve; merges embedded UI into Console router when ui_enabled
|   |-- lib.rs                  # Module declarations for the single binary
|   |-- http/
|   |   |-- mod.rs              # Public API-key data-plane router (/v1/*)
|   |   |-- console.rs          # Separate JWT-authenticated Console router (/console/v1/*)
|   |   `-- console_ui.rs       # Embedded SPA assets + SPA fallback + cache/security headers (embedded-console-ui feature only)
|   |-- admission/              # Process-local RPM, concurrency, and soft quota admission
|   |-- domain/                 # API formats, compiled routing, credentials, request-log events
|   |-- runtime_config/         # TOML deserialization and ArcSwap configuration snapshots; [console].ui_enabled validation
|   |-- observability/          # tracing-subscriber initialization
|   |-- application/            # Proxy, Console auth, control-plane publication, request-log sink
|   |-- request_log_journal.rs  # Versioned safe request-log payload encoding
|   |-- request_log_spool.rs    # CRC-protected local append log and checkpoints
|   |-- routing/                # Priority/weight selection and passive health state
|   |-- transforms/             # Compiled constrained JSON/header/SSE transform DSL
|   |-- upstream/               # Reused reqwest clients, proxy policy, timeout resolution
|   |-- persistence/            # SQLx repositories, Console auth/session state, control-plane mutations, logs
|   `-- workers/                # Snapshot reload plus spool ingestion, DB projection, and settlement
|-- migrations/                 # PostgreSQL control-plane and log schema migrations
|-- tests/                      # Local, PostgreSQL, proxy, streaming, real-upstream, and console-spec integration tests
|-- tools/forwarding-perf/      # Manual isolated forwarding benchmark orchestrator, Mock LLM, load client, and report generator
|-- web/console/                # React 19 + TypeScript + Vite + Tailwind v4 + shadcn/ui (Radix) SPA, the Console web UI
|   |-- src/api/                # Typed Console client, session store, MSW/test helpers, generated OpenAPI types (generated/console-v1.d.ts)
|   |-- src/app/                # Providers, router, layouts, theme
|   |-- src/features/           # Feature modules (auth, profile, sessions, api-keys, request-logs, admin control plane)
|   |-- src/components/         # shadcn/ui primitives (ui/) and shared app components (shared/)
|   |-- src/test/               # vitest setup, MSW server, deterministic fixtures
|   |-- e2e/                    # Playwright browser smoke tests (API mocked at network layer)
|   `-- vite.e2e.config.ts      # HTTP-only vite config for Playwright's readiness probe
|-- docs/PRD.md                 # Canonical product and architecture blueprint (Chinese)
|-- docs/openapi/console-v1.yaml # Authoritative OpenAPI spec for the Console API (TS types are generated from it)
|-- docs/console-ui-design.md    # Console Web UI architecture and implementation plan
|-- docs/forwarding-performance.md # Manual performance-harness design, profiles, metrics, and safety model
|-- config/                     # Ignored local runtime config and JWT keys
|-- config.example.toml         # Canonical configuration template
|-- docker-compose.yml          # Development PostgreSQL service only
`-- Cargo.toml                  # Workspace plus production package metadata, MSRV, features, and dependency source of truth
```

## Build, Test, and Development

Run Rust commands from the repository root; run frontend commands with
`pnpm --dir web/console <script>` (Node/pnpm are build/test/dev tooling only,
not a production runtime).

```bash
# --- Rust: check, format, lint, test ---
cargo check
cargo fmt --check
cargo clippy --all-targets               # also run with --features embedded-console-ui when that path changes
cargo test                                # unit + local/PostgreSQL integration (needs `docker compose up -d`)
cargo test --lib console_ui               # embedded-UI serving tests (needs --features embedded-console-ui + built web/console/dist)
cargo test --test console_spec_integration # OpenAPI spec/Console-API drift tests (needs PostgreSQL)
cargo test --package ai-gateway-perf       # Fast unit tests for the manual performance tooling; does not run a benchmark
cargo clippy --package ai-gateway-perf --all-targets # Lint the separate performance-tool package

# --- Rust: run ---
cargo run                                 # loads ignored ./config/config.toml
cargo run -- ./config/other-config.toml   # explicit TOML path
cargo run --release --features embedded-console-ui   # production binary with embedded Console UI

# One-time first Console administrator; password is read only from stdin
cargo run -- bootstrap-admin --email admin@example.com --display-name "Initial Admin" --password-stdin < password.txt

# Start the development PostgreSQL service when persistence work needs it
docker compose up -d

# --- Manual performance harness: run only when the user explicitly requests it ---
./scripts/run-forwarding-perf.sh --profile quick
./scripts/run-forwarding-perf.sh --profile standard

# --- Frontend (web/console): checks, tests, build ---
pnpm --dir web/console typecheck          # tsc --noEmit (strict)
pnpm --dir web/console lint               # oxlint (5 shadcn fast-refresh warnings are acceptable)
pnpm --dir web/console test               # vitest component tests (jsdom + MSW)
pnpm --dir web/console e2e                # Playwright browser tests (installs Chromium; runs an HTTP dev server on :5174)
pnpm --dir web/console build              # tsc -b && vite build -> web/console/dist (embedded by rust-embed)
pnpm --dir web/console generate:api       # regenerate src/api/generated/console-v1.d.ts from docs/openapi/console-v1.yaml
pnpm --dir web/console generate:api:check # CI drift gate: fails if generated types differ from the committed spec
```

### Console UI run modes

- **Development:** `cargo run` (Console API on `127.0.0.1:3001`) plus `pnpm --dir web/console dev` (HTTPS on `https://console.localhost:5173`, proxying `/console/v1/*` to the gateway). `ui_enabled` is irrelevant here; the Vite server is a dev Node process. Prerequisites: `[console]` enabled with an `[auth]` JWT key pair, bootstrap admin exists.
- **Production:** build `web/console/dist`, then `cargo build --release --features embedded-console-ui` and set `[console].ui_enabled = true`. The binary serves the SPA from the Console listener only. In debug builds `rust-embed` reads `web/console/dist` from disk at runtime, so dist must still exist; release builds embed it.

The Rust test suite contains unit tests and local/PostgreSQL integration
tests. `cargo test` is the baseline Rust verification. The ignored
`tests/real_upstream/` contains paid external calls and must only run via
`./scripts/run-real-upstream-smoke.sh`; see `docs/real-upstream-smoke.md`.
**Any change to the forwarding path must also run this real-upstream script
before completion.** It serially verifies both `/v1/chat/completions` and
`/v1/responses`, with non-streaming and SSE requests. There is no CI workflow
yet. `docker-compose.yml` provides PostgreSQL only—the application is not
containerized.

The separate forwarding performance harness is documented in
`docs/forwarding-performance.md`. It creates a random throwaway database,
starts a Mock LLM and a fresh release gateway process, runs direct and proxied
loads, and writes reports under ignored `target/perf/`. **Never run
`scripts/run-forwarding-perf.sh` unless the user explicitly asks for a
performance run.** Building the tool or running
`cargo test --package ai-gateway-perf` is safe and does not execute load.

## Configuration Rules

- The normal serve command loads the first CLI argument as TOML, defaulting to ignored `./config/config.toml` in the current working directory (`src/main.rs`). It does not use an XDG configuration directory. `bootstrap-admin` is a separate one-time CLI subcommand and requires `--password-stdin`. There is no dotenv support or automatic local-override merge.
- Keep `config.example.toml` and the deserialization types in `src/runtime_config/mod.rs` synchronized whenever configuration changes.
- `./config/config.toml` and Console JWT key files under `./config/` are ignored. A different current-directory TOML path can be passed explicitly. The binary never loads `.env` files. The sole exception is the ignored `.env.real-upstream` file, which `scripts/run-real-upstream-smoke.sh` may source for opt-in test credentials.
- Configuration changes intended for live reload should preserve the immutable-snapshot pattern: construct a complete `AppConfig`, then replace it atomically through `RuntimeConfig`.
- `[console].ui_enabled = true` mounts the embedded Console UI on the Console listener, but requires building with the `embedded-console-ui` cargo feature (and a built `web/console/dist`). Setting `ui_enabled = true` without the feature compiled in is rejected at startup with a `ConfigError` (`src/runtime_config/mod.rs`). The UI is served only from the Console listener, never from the public `/v1/*` data-plane listener.

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

### Console API and embedded UI

- The Console API lives at `/console/v1/*` (`src/http/console.rs`); the embedded SPA is merged **after** the API router in `main.rs` so explicit `/console/v1/*` routes always take precedence over the SPA fallback. An unmatched `/console/v1/*` path returns a JSON 404, never `index.html`.
- The SPA fallback only answers `GET`/`HEAD`. Fingerprinted `/assets/*` are served with immutable cache; `index.html` and root files use `no-cache` and carry security headers (`src/http/console_ui.rs`).
- UI resources must never be reachable from the public `/v1/*` listener. There is no SSR and no Node process in production; Node/pnpm are build and dev tooling only.
- Access tokens live in browser memory only (never `localStorage`); the rotating refresh token stays in an `HttpOnly; Secure; SameSite=Lax` cookie. The Console listener is meant to sit behind an HTTPS reverse proxy.
- The OpenAPI spec at `docs/openapi/console-v1.yaml` is the single source of truth for Console request/response shapes. The frontend imports generated types from `web/console/src/api/generated/console-v1.d.ts`; `web/console/src/api/types.ts` is only a re-export shim (plus the client-side `ControlPlaneLists` aggregate). Change the spec, run `pnpm --dir web/console generate:api`, and commit both; `generate:api:check` is the drift gate.

## Common Change Workflows

### Add a configuration setting

1. Add the field to the appropriate TOML-deserialized type in `src/runtime_config/mod.rs`.
2. Add its documented default to the tracked `config.example.toml`; do not commit local `./config/config.toml` or JWT files under `./config/`.
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

### Add or change a Console API endpoint or contract

1. Edit `docs/openapi/console-v1.yaml` (the authoritative contract) for request/response shapes, status codes, and ETag semantics.
2. Run `pnpm --dir web/console generate:api` to regenerate `web/console/src/api/generated/console-v1.d.ts`, then update call sites that import from `@/api/types` (a re-export shim). Keep the spec's enum values aligned with the backend (e.g. `SelectionStrategy` must be `weighted_random`/`weighted_round_robin`).
3. Implement/adjust the Rust handler in `src/http/console.rs` and business logic in `src/application`; enforce ownership/admin in repository SQL, not in handler bodies.
4. Add a spec/implementation pin in `tests/console_spec_integration.rs` for shape, error body, ETag/`If-Match`, or one-time-secret behavior where applicable, then run `cargo test --test console_spec_integration`.
5. Add frontend page/tests as needed (see below) and run `pnpm --dir web/console generate:api:check` plus the frontend checks.

### Change the Console web UI

1. Work inside `web/console/` (React 19 + TypeScript + Vite + Tailwind v4 + shadcn/ui). Keep `src/api/types.ts` as a re-export shim; do not hand-edit `src/api/generated/console-v1.d.ts`.
2. For forms driven by Radix `Select` with `react-hook-form`, every `Select`-backed field must be seeded in `useForm({ defaultValues })` (or `register`ed), or `reset()` values are absent from validation and the form silently fails to submit. See the api-key and user detail pages for the pattern.
3. Construct the React Query `QueryClient` per `AppProviders` mount (it already is); do not make it a module singleton, or cached query state leaks across mounts (HMR and component tests).
4. Add a vitest component test (deterministic `src/test/fixtures.ts` + `src/test/msw.ts` handlers use relative paths) and/or a Playwright e2e flow in `e2e/`. Component tests live under `src/**`; e2e specs live under `e2e/` and are excluded from vitest.
5. Run `pnpm --dir web/console typecheck && pnpm --dir web/console lint && pnpm --dir web/console test && pnpm --dir web/console build`. If the Rust embedding path changed, also `cargo test --features embedded-console-ui --lib console_ui`.

### Change request forwarding

1. Add or update deterministic local and PostgreSQL tests as appropriate.
2. Run `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo test`.
3. Run `./scripts/run-real-upstream-smoke.sh` before considering the change complete. It requires the ignored `.env.real-upstream` file and makes paid calls to both configured upstream formats.
4. Do not print, commit, or copy credentials from `.env.real-upstream` into TOML, source, tests, or logs.

## Gotchas

1. **The PRD is not the runtime.** It includes later roadmap capabilities such as active health checks, cross-instance coordination, and generic retries. Confirm an API exists before integrating with it.
2. **Public and Console routes are separate.** `src/http/mod.rs` exposes the API-key data plane; `src/http/console.rs` is bound by `main` only when `[console].enabled` is set. Console is intended for an HTTPS reverse proxy, and `admin` is a role rather than a route namespace or static bearer token.
3. **Migrations are authoritative.** Add ordered SQL migrations for schema changes; there is no SQLx offline cache.
4. **`./config/config.toml` is auto-loaded.** It is intentionally ignored by Git; invoke `cargo run -- ./config/other-config.toml` only when using another file.
5. **Runtime configuration is TOML-only.** Do not add dotenv loading to the binary. Console Ed25519 keys are supplied by protected file paths, not TOML values. `.env.real-upstream` is test-script-only and must remain ignored.
6. **Embedded UI needs a built dist and the feature.** `[console].ui_enabled = true` is rejected at startup unless the binary is built with `--features embedded-console-ui`. In debug builds `rust-embed` reads `web/console/dist` from disk at runtime, so `pnpm --dir web/console build` must have run; release builds embed dist into the binary. Without `web/console/dist`, the UI router serves a placeholder error instead of `index.html`.
7. **The OpenAPI spec is the Console API source of truth.** `docs/openapi/console-v1.yaml` drives `web/console/src/api/generated/console-v1.d.ts` via `pnpm --dir web/console generate:api`. `generate:api:check` fails if the generated file drifts from the committed spec, so regenerate and commit both together. Never hand-edit the generated file.
8. **react-hook-form `Select` fields must be seeded.** Radix `Select` components driven by `form.watch`/`form.setValue` are not registered with react-hook-form unless their values appear in `useForm({ defaultValues })`. Without `defaultValues`, `form.reset()` values are missing from validation and submitting without re-selecting the dropdown silently fails (and there is no `FieldError` render). Always seed every `Select`-backed enum/optional field.
9. **Frontend TS is strict and `erasableSyntaxOnly`.** `verbatimModuleSyntax`, `noUnusedLocals`, `noUnusedParameters`, and `erasableSyntaxOnly` are on: use `import type`, no TypeScript enums, no unused locals/params, and no runtime-only TS syntax. oxlint (not ESLint) is the linter; the 5 shadcn fast-refresh warnings are expected and acceptable.
10. **Component tests vs. e2e are scoped.** vitest `include` is `src/**/*.{test,spec}.{ts,tsx}` and `exclude`s `e2e`, so Playwright specs (`e2e/*.spec.ts`) are not collected by vitest. e2e uses `vite.e2e.config.ts` (plain HTTP on `127.0.0.1:5174`) because Playwright's webServer readiness probe cannot ignore the dev server's self-signed HTTPS.
11. **Performance runs are always opt-in.** `tools/forwarding-perf/` is a separate workspace package and its unit tests are lightweight, but `scripts/run-forwarding-perf.sh` starts release processes and generates sustained concurrent traffic. Do not invoke either the `quick` or `standard` profile without an explicit user request. The harness must keep using random `ai_gateway_perf_*` databases and must never point its admin URL at the normal `ai_gateway` database.
12. **Request-log durability has two backlogs.** Production uses a process-unique local spool, then `request_log_ingest`, then the indexed `request_logs` table and settlement. Notification-queue fullness is harmless, but spool append errors are not. Never checkpoint before COPY commit or delete ingress rows before final-table persistence succeeds; both replay paths rely on UUID idempotency.

## Code Style

- Use standard Rust formatting (`cargo fmt`) and linting (`cargo clippy`).
- Retain concise module-level `//!` documentation for module responsibilities.
- Model domain failures with typed errors (`thiserror` is available); propagate process-boundary failures from `main` as appropriate.
- Keep sensitive material in the intended secrecy/zeroization boundary. The PRD explicitly permits administrators and users to view stored client and upstream API keys; do not silently change that product behavior without a coordinated design change.

### Frontend (`web/console/`)

- React 19 + TypeScript (strict), Vite, Tailwind CSS v4, shadcn/ui (Radix) components kept as version-controlled source under `src/components/ui/`. Use pnpm; the lockfile and `components.json` are committed, `node_modules/` and `dist/` are ignored.
- Model form validation with `zod` + `react-hook-form` (`zodResolver`); seed every `Select`-backed field in `defaultValues`. Server data fetching/mutation via `@tanstack/react-query`; optimistic-concurrency edits send `If-Match` from the GET `ETag` and render a 409 reload toast.
- API access goes through `src/api/client.ts` (`Bearer` access token, single-flight refresh, `ETag`/`If-Match`); types come from `@/api/types` (a re-export of the generated OpenAPI types). Access tokens are memory-only; never persist them to `localStorage`.
- Routes are lazy-loaded via `React.lazy` + `Suspense` for per-route code splitting; auth/admin guards are `RequireAuth`/`RequireAdmin`. The `QueryClient` is created per `AppProviders` mount, not as a module singleton.
- Formatting/linting: `pnpm --dir web/console typecheck` (tsc) and `pnpm --dir web/console lint` (oxlint). No ESLint. The 5 shadcn fast-refresh lint warnings are acceptable.
- Tests: vitest + jsdom + MSW component tests under `src/**` with deterministic `src/test/fixtures.ts` and relative-path `src/test/msw.ts` handlers; Playwright browser e2e under `e2e/`.

## Quick Reference

| Need | Source of truth |
|---|---|
| Package/MSRV/dependencies | `Cargo.toml` |
| Product architecture and constraints | `docs/PRD.md` |
| Supported client formats | `src/domain/api_format.rs` |
| Public data-plane route registry | `src/http/mod.rs` |
| Console API route registry | `src/http/console.rs` |
| Embedded UI serving/fallback/cache | `src/http/console_ui.rs` (feature-gated) |
| Startup, config-path, UI merge behavior | `src/main.rs` |
| TOML config schema and snapshot API | `src/runtime_config/mod.rs` |
| Example configuration | `config.example.toml` |
| Console API contract (request/response shapes) | `docs/openapi/console-v1.yaml` |
| Console UI generated TypeScript types | `web/console/src/api/generated/console-v1.d.ts` (regenerate via `pnpm --dir web/console generate:api`) |
| Console UI architecture and implementation plan | `docs/console-ui-design.md` |
| Console/JWT design and execution plan | `docs/console-auth-refactor-plan.md` |
| Current operational API documentation | `docs/mvp-usage.md` |
| Console spec/implementation drift tests | `tests/console_spec_integration.rs` |
| Frontend package/scripts | `web/console/package.json` |
| Frontend checks and testing guide | `web/console/README.md` |
| Frontend license attribution | `web/console/NOTICES.md` |
| Local PostgreSQL service | `docker-compose.yml` |
| Opt-in real upstream test | `docs/real-upstream-smoke.md` and `scripts/run-real-upstream-smoke.sh` |
| Opt-in forwarding performance harness | `docs/forwarding-performance.md`, `tools/forwarding-perf/`, and `scripts/run-forwarding-perf.sh` |
| Request-log durability pipeline | `docs/request-log-durability.md`, `src/request_log_spool.rs`, and `src/workers/durable_request_log.rs` |
