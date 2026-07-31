# AGENTS.md - ai-gateway

> Operational context for coding agents. Start with `docs/README.md` for the
> document map and verify current behavior in code, tests, migrations, and
> machine-readable contracts.

## What is ai-gateway?

`ai-gateway` is a single-binary Rust production service intended to forward LLM requests in the OpenAI Chat Completions, Responses, and Images formats. It uses Axum/Tokio for HTTP, reqwest for upstream requests, PostgreSQL/SQLx for persistence, and `ArcSwap` for immutable runtime configuration snapshots. Rust 2024 with MSRV 1.92 is required (`Cargo.toml`); `rust-toolchain.toml` pins Rust 1.97.1 for normal development and release builds. The Cargo workspace also contains the development-only `ai-gateway-perf` package under `tools/forwarding-perf/`; it is never linked into the production binary.

The project is licensed under `AGPL-3.0-only`. Third-party license texts and
attributions that must accompany binary redistribution live in `LICENSES/`
and `web/console/NOTICES.md`.

The implemented backend includes OpenAI-compatible Chat Completions, HTTP
Responses, Responses WebSocket, non-streaming JSON Images generation, and
non-streaming multipart Images edit proxy routes, including ordinary
OpenAI-compatible and Codex OAuth Images channels, plus PostgreSQL-backed
control-plane snapshots, a separate JWT-authenticated Console API with
`user`/`admin` roles, constrained transforms, streaming/SSE/WebSocket
forwarding, passive health, admission controls, durable spooled request logs,
and reusable upstream clients. A React + TypeScript
Console web UI lives under `web/console/` and can be embedded into the binary
as static assets via the optional `embedded-console-ui` cargo feature, served
only from the Console listener. `docs/development/architecture.md` describes
the current architecture; `docs/development/product-blueprint.md` preserves
product direction and design background but never overrides the runtime.

The Console API contract is an authoritative OpenAPI spec at
`docs/openapi/console-v1.yaml`; the frontend's TypeScript types are generated
from it (never hand-edited). There is no SSR or long-running Node service in
production — Node is build/test/dev tooling only. `Dockerfile` builds the UI
and embeds it into the release binary; `docker-compose.prd.yaml` can deploy the
Gateway and PostgreSQL from either a registry image or a local build.

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
|   |-- upstream/               # Reused reqwest clients, Responses WebSocket pool/dialer, proxy policy, timeout resolution
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
|-- docs/
|   |-- README.md                # Documentation map and source precedence
|   |-- documentation-standard.md # Required categories, status labels, links, and update matrix
|   |-- user/                    # Current usage, production configuration, and deployment
|   |-- development/             # Current architecture, design records, testing, performance, and releases
|   |-- reference/               # External OpenAI semantics and gateway compatibility boundaries
|   |-- archive/                 # Historical MVP plans; never current behavior
|   `-- openapi/console-v1.yaml  # Authoritative Console API spec; drives generated TS types
|-- .github/                    # SHA-pinned path-aware CI, reusable quality, security, release workflows, and Dependabot updates
|-- config/                     # Ignored runtime config, DB password, JWT keys
|-- config.example.toml         # Canonical configuration template
|-- deploy/compose/             # Container-specific TOML and Compose environment templates
|-- deploy/docker/              # Runtime container entrypoint and privilege drop
|-- deploy/postgres/            # Compose-only PostgreSQL initialization helpers
|-- Dockerfile                  # Multi-stage UI + Rust production image
|-- docker-compose.yml          # Tuned single-node PostgreSQL baseline (not HA)
|-- docker-compose.prd.yaml     # Full Gateway + PostgreSQL single-host production stack
|-- CHANGELOG.md                # Dated Keep-a-Changelog release notes
|-- LICENSE                     # GNU Affero General Public License v3.0-only
|-- LICENSES/                   # Committed third-party license texts shipped with releases
|-- rust-toolchain.toml         # Exact default development/release Rust toolchain
`-- Cargo.toml                  # Workspace plus production package metadata, MSRV, features, and dependency source of truth
```

## Build, Test, and Development

Run Rust commands from the repository root; run frontend commands with
`pnpm --dir web/console <script>` (Node/pnpm are build/test/dev tooling only,
not a production runtime). Plain `cargo` commands use Rust 1.97.1 from
`rust-toolchain.toml`; invoke Rust 1.92.0 explicitly for MSRV validation.

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
cargo +1.92.0 check --locked --workspace --all-targets # MSRV compile gate
cargo +1.92.0 test --locked --workspace    # MSRV test gate

# --- Rust: run ---
cargo run                                 # loads ignored ./config/config.toml
cargo run -- ./config/other-config.toml   # explicit TOML path
cargo run --release --features embedded-console-ui   # production binary with embedded Console UI

# One-time first Console administrator; password is read only from stdin
cargo run -- bootstrap-admin --email admin@example.com --display-name "Initial Admin" --password-stdin < password.txt

# First use: generate the ignored database password file, then start PostgreSQL
mkdir -p ./config
openssl rand -hex 32 > ./config/postgres-password
chmod 600 ./config/postgres-password
docker compose up -d

# --- Container/release validation ---
docker compose -f docker-compose.prd.yaml config --quiet
docker build -t ai-gateway:local .
docker run --rm ai-gateway:local --version
./scripts/check-release-version.sh 0.1.0
./scripts/verify-release.sh 0.1.0

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
`./scripts/run-real-upstream-smoke.sh`; see `docs/development/real-upstream-smoke.md`.
**Any change to the forwarding path must also run this real-upstream script
before completion.** It serially verifies `/v1/chat/completions` and
`/v1/responses`, with non-streaming and SSE requests plus Responses WebSocket.
Images generation/edit currently rely on deterministic proxy integration
tests; the paid smoke does not issue an image request.
GitHub Actions workflows under `.github/workflows/` run path-aware ordinary
CI, reusable Rust/Console/E2E quality gates, CodeQL security scanning, and the
tag release path. Every PR emits the stable `ci-gate`; Markdown-only changes
run documentation validation instead of skipping CI.
`docker-compose.yml` remains the PostgreSQL-only development/baseline stack;
`docker-compose.prd.yaml` adds the containerized Gateway. Neither stack
provides HA, PITR, or backups.

The separate forwarding performance harness is documented in
`docs/development/forwarding-performance.md`. It creates a random throwaway database,
starts a Mock LLM and a fresh release gateway process, runs direct and proxied
loads, and writes reports under ignored `target/perf/`. **Never run
`scripts/run-forwarding-perf.sh` unless the user explicitly asks for a
performance run.** Building the tool or running
`cargo test --package ai-gateway-perf` is safe and does not execute load.

## Configuration Rules

- The normal serve command loads the first CLI argument as TOML, defaulting to ignored `./config/config.toml` in the current working directory (`src/main.rs`). It does not use an XDG configuration directory. `bootstrap-admin` is a separate one-time CLI subcommand and requires `--password-stdin`. There is no dotenv support or automatic local-override merge.
- Keep `config.example.toml`, `deploy/compose/config.example.toml`, and the deserialization types in `src/runtime_config/mod.rs` synchronized whenever configuration changes. The container template deliberately differs only in listener/database/spool/secret paths and enabled embedded Console settings.
- The canonical Compose setup reads its password from ignored
  `./config/postgres-password`; do not reintroduce an inline default password.
- `./config/config.toml` and Console JWT key files under `./config/` are ignored. A different current-directory TOML path can be passed explicitly. The binary never loads `.env` files. The sole exception is the ignored `.env.real-upstream` file, which `scripts/run-real-upstream-smoke.sh` may source for opt-in test credentials.
- Configuration changes intended for live reload should preserve the immutable-snapshot pattern: construct a complete `AppConfig`, then replace it atomically through `RuntimeConfig`.
- `[console].ui_enabled = true` mounts the embedded Console UI on the Console listener, but requires building with the `embedded-console-ui` cargo feature (and a built `web/console/dist`). Setting `ui_enabled = true` without the feature compiled in is rejected at startup with a `ConfigError` (`src/runtime_config/mod.rs`). The UI is served only from the Console listener, never from the public `/v1/*` data-plane listener.

## Documentation Rules

- Follow `docs/documentation-standard.md` for categories, status labels,
  relative links, source precedence, and the change synchronization matrix.
- Put current user-observable behavior in `docs/user/`, maintainer design and
  workflows in `docs/development/`, third-party API semantics in
  `docs/reference/`, and obsolete milestone material in `docs/archive/`.
- `README.md` and `README.zh-CN.md` are project overview and quick-start
  documents. Link to detailed user docs instead of duplicating long
  operational sections in new locations.
- External reference documents must link authoritative sources, record the
  last verification date, and distinguish external behavior from gateway
  guarantees. Do not copy complete third-party API references.
- When moving a document, search the entire repository for the old path,
  including code comments, scripts, Compose files, package metadata, and
  release packaging.
- Documentation-only changes run `git diff --check` and
  `python3 scripts/check-docs.py`. If source comments or scripts change, also
  run the narrow formatter or syntax check appropriate to those files.

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

### Load-bearing rules

- Support `OpenAiChatCompletions`, `OpenAiResponses`, and `OpenAiImages` (`src/domain/api_format.rs`). Keep their validation and routing paths separate: never fall back or transform between formats. `OpenAiImages` exposes JSON `POST /v1/images/generations` and multipart `POST /v1/images/edits`; image streaming and public JSON/data-URL edits are not implemented.
- `model_rules`, channel groups, and channels must agree on `api_format`; a model rule is unique by `(client_model, api_format)`.
- Parse a request only as far as necessary to obtain `model`; absent an enabled transform or model alias, forward the original request bytes without reserialization.
- Keep multipart Images edits behind `ReplayableRequestBody::{Memory, TempFile}` and the dedicated `image_edit_*` limits. Never raise the global JSON body limit to accommodate images, require complete input images/data-URL JSON to remain in memory beyond the configured threshold, retain named upload files, or put multipart values in logs/errors/audit.
- Multipart edit request JSON transforms fail closed; ordinary connectors replay exact bytes or rebuild only the model field, while Codex adapts at most five images to streamed base64 data URLs and rejects masks/unverified fields.
- Keep the fixed transform order: template defaults → channel overrides → upstream authentication. Configurable transforms must not alter protected or hop-by-hop headers.
- A Codex OAuth logical credential belongs to one `connector_pools` record and projects through
  `codex_oauth_credential_channels` to separate Responses and Images managed channels. Preserve the
  legacy Responses channel/credential ID, share token/quota/proxy state, keep format health and
  authorization isolated, and never auto-enable the paired Images group or grant Images access.
- Stream upstream responses instead of buffering them. Do not retry or switch channels after sending response headers or any response byte to the client.
- Responses WebSocket accepts `GET /v1/responses` upgrades only. Authenticate
  the upgrade, then treat each sequential `response.create` as its own
  admitted, routed, logged request; never multiplex concurrent Responses on
  one socket.
- Keep each in-flight `response.create` exclusively pinned to one eligible
  upstream socket because incremental `previous_response_id` cache state is
  connection-local. Return only clean sockets to the session-isolated pool
  after a successful terminal event, key pool entries by API key, handshake
  identity, channel, network policy, target, and final headers, and never retry
  after sending a WebSocket request message upstream.
- Track upgraded Responses WebSocket tasks independently from Hyper connection
  futures: reject new upgrades during shutdown, drain the current logical
  request within the configured grace period, then force-close any remainder.
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
4. After creating ignored `./config/postgres-password` as shown above, start PostgreSQL with `docker compose up -d` and verify migrations and repository behavior.

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
3. Run `./scripts/run-real-upstream-smoke.sh` before considering the change complete. It requires the ignored `.env.real-upstream` file and makes paid Chat Completions and Responses calls; Images changes also require deterministic proxy integration coverage because the paid smoke does not yet issue image requests.
4. Do not print, commit, or copy credentials from `.env.real-upstream` into TOML, source, tests, or logs.

For Responses WebSocket changes, also run
`cargo test --locked --test websocket_integration`; cover sequential reuse,
pool isolation, transforms, and configured outbound proxies.

### Prepare a release

1. Follow `docs/development/releasing.md`; keep the versions in Cargo, the Console package,
   production Compose defaults, and `CHANGELOG.md` synchronized.
2. Run `./scripts/check-release-version.sh <version>`, then
   `./scripts/verify-release.sh <version>`.
3. Commit the release changes on `main`, then use
   `./scripts/release.sh <version> --push` for an annotated `v<version>` tag and
   atomic main/tag push.
4. Never move or reuse a published tag. Publish a new patch release instead.

### Add or reorganize documentation

1. Choose the audience and category using `docs/documentation-standard.md`.
2. Add a status marker; external references also need an authoritative source
   and verification date.
3. Update `docs/README.md` and the category index when the navigation changes.
4. Search the entire repository for moved paths and update release packaging
   if distributed documents changed.
5. Run `git diff --check` and `python3 scripts/check-docs.py`.

## Gotchas

1. **The product blueprint is not the runtime.** It includes roadmap and historical language. Use `docs/development/architecture.md`, code, tests, migrations, and OpenAPI for current behavior.
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
13. **Container secrets and spool directories are prepared before privilege drop.** Local Compose file-backed secrets and named volumes may retain host ownership/mode. `deploy/docker/entrypoint.sh` starts as root, copies config and secrets into a private tmpfs, fixes the request-log and Images edit spool ownership/modes, then executes the Gateway as UID/GID 10001. Do not bypass that entrypoint in production.
14. **Release tags are deployment inputs.** `.github/workflows/release.yml` is tag-triggered. Version drift or a missing dated Changelog entry fails the release. Verification runs read-only, GHCR publication has only package write permission, and GitHub Release publication has only contents write permission. Public repositories also publish image provenance attestations; private repositories skip that step.
15. **GitHub Actions references are immutable.** External Actions are pinned to full commit SHAs; version-tagged Actions are updated through `.github/dependabot.yml`. Do not replace them with mutable major tags or branches. The Rust toolchain Action is pinned to a reviewed `stable` branch commit and requires periodic manual refresh.
16. **PR workflows must not write caches.** `.github/workflows/reusable-quality.yml` and `.github/workflows/ci.yml` allow cache writes only for `main` or tag-triggered Release runs. PR jobs may restore default-branch/Release caches but must not add `cache-to`, `actions/cache/save`, or an unconditional Rust cache save.
17. **`ci-gate` is the required stable check.** Keep ordinary CI triggered for every PR, route Markdown-only changes through `scripts/check-docs.py`, and make the final `ci-gate` fail for every selected job result other than `success`/`skipped`. The default-branch ruleset requires this exact check name.
18. **License metadata and redistribution notices move together.** The project license is `AGPL-3.0-only`; keep both Cargo package manifests, `web/console/package.json`, README license sections, Docker OCI labels, and release archives synchronized. The embedded Geist font remains OFL-1.1 and its committed license plus `web/console/NOTICES.md` must stay in binary distributions.
19. **Responses WebSocket state is connection-local.** Do not build a
    multiplexing pool that moves sequential `response.create` messages between
    arbitrary upstream sockets. Keep one request in flight, prefer the same
    session-isolated connection for `previous_response_id`, and discard any
    socket with an incomplete response, terminal error, queued residual
    message, or expired pool lifetime.

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
| Default Rust toolchain and MSRV policy | `rust-toolchain.toml` and `docs/development/rust-toolchain-policy.md` |
| Package/MSRV/dependencies | `Cargo.toml` |
| Documentation map and rules | `docs/README.md` and `docs/documentation-standard.md` |
| Documentation validation | `python3 scripts/check-docs.py` |
| Current architecture and constraints | `docs/development/architecture.md` |
| Product direction and design background | `docs/development/product-blueprint.md` |
| Supported client formats | `src/domain/api_format.rs` |
| Public data-plane route registry | `src/http/mod.rs` |
| Images multipart capture/replay and Codex edit adaptation | `src/application/request_body.rs` |
| Responses WebSocket proxy and pooling | `src/application/proxy/websocket.rs` and `src/upstream/websocket.rs` |
| Console API route registry | `src/http/console.rs` |
| Embedded UI serving/fallback/cache | `src/http/console_ui.rs` (feature-gated) |
| Startup, config-path, UI merge behavior | `src/main.rs` |
| TOML config schema and snapshot API | `src/runtime_config/mod.rs` |
| Example configuration | `config.example.toml` |
| Container configuration template | `deploy/compose/config.example.toml` |
| Full production Compose | `docker-compose.prd.yaml` and `docs/user/production-deployment.md` |
| Release process/version checks | `docs/development/releasing.md`, `scripts/release.sh`, and `scripts/check-release-version.sh` |
| CI, security, cache, and release automation | `docs/development/continuous-integration.md`, `.github/workflows/`, and `.github/dependabot.yml` |
| Project and third-party licensing | `LICENSE`, `LICENSES/`, and `web/console/NOTICES.md` |
| Console API contract (request/response shapes) | `docs/openapi/console-v1.yaml` |
| Console UI generated TypeScript types | `web/console/src/api/generated/console-v1.d.ts` (regenerate via `pnpm --dir web/console generate:api`) |
| Console UI architecture and implementation plan | `docs/development/console-ui.md` |
| Console/JWT design and execution plan | `docs/development/console-auth.md` |
| Current operational API documentation | `docs/user/operations.md` |
| OpenAI compatibility and external semantics | `docs/reference/` |
| Images staged design and Codex projection | `docs/development/openai-images.md` |
| Codex OAuth connector architecture | `docs/development/codex-oauth-connector.md` |
| Codex Responses WebSocket source study | `docs/reference/codex-responses-websocket.md` |
| Console spec/implementation drift tests | `tests/console_spec_integration.rs` |
| Frontend package/scripts | `web/console/package.json` |
| Frontend checks and testing guide | `web/console/README.md` |
| Frontend license attribution | `web/console/NOTICES.md` |
| Local PostgreSQL service | `docker-compose.yml` |
| Opt-in real upstream test | `docs/development/real-upstream-smoke.md` and `scripts/run-real-upstream-smoke.sh` |
| Opt-in forwarding performance harness | `docs/development/forwarding-performance.md`, `tools/forwarding-perf/`, and `scripts/run-forwarding-perf.sh` |
| Request-log durability pipeline | `docs/development/request-log-durability.md`, `src/request_log_spool.rs`, and `src/workers/durable_request_log.rs` |
