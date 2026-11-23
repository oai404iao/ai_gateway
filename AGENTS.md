# ai-gateway contributor guide

> Status: Current.

The Rust gateway serves the public data plane and a separate JWT-authenticated Console. The optional Console UI is a static React build; Node is development tooling. Start with the [development guide](docs/development/getting-started.md) for setup and the [architecture](docs/development/architecture.md) for the request lifecycle.

## Sources to edit

- **Public requests:** [request-allowlists.json](docs/reference/request-allowlists.json) and `src/request_policy.rs` own the client Header and top-level body policy. Filter group Fast mode after the body allowlist, before logging, pricing, transforms and plugin preparation. Reapply shared outbound Header guards before dispatch. Provider-specific privacy rules live in the external plugin, after administrator transforms.
- **Routing and transport:** Each selected channel/model candidate belongs to an operation; the priced client model owns billing. Keep in-flight Responses WebSocket requests on their selected socket. Use the [Images guide](docs/development/openai-images.md) when changing multipart handling and disk replay.
- **Plugins:** `general` is built in. Native plugin commands and the shared-library boundary are defined in the [SDK](crates/connector-sdk/docs/commands.md). Plugin generations own immutable settings; the host owns authentication, transport, quotas and settlement. Installations do not grant routing access.
- **Console contracts:** Edit [console-v1.yaml](docs/openapi/console-v1.yaml), run `pnpm --dir web/console generate:api`, and commit the generated types with the spec. Use the typed API client. Check role and resource ownership in repository queries. Keep refresh tokens in the secure cookie and access tokens in memory.
- **Console UI:** Follow the [frontend guide](web/console/README.md) for React, shadcn/Base UI and tests. Seed Select-backed fields in React Hook Form `defaultValues` so untouched values survive validation. Text changes cover both locales.
- **Configuration:** Keep the types in `src/runtime_config/` and both the [host](config.example.toml) and [container](deploy/compose/config.example.toml) TOML templates in sync. Database policy changes publish one complete compiled runtime snapshot. The data plane reads that snapshot, not the database.
- **Storage and recovery:** See [database architecture](docs/development/database-architecture.md), [independent metering](docs/development/independent-metering.md) and [SQLite lifecycle](docs/development/sqlite-lifecycle.md). Request logs are a projection, not the financial ledger. Spool checkpoints, unique settlement receipts and uncertain external effects have distinct recovery rules. Storage starts from the fresh baseline; validate current reopen/restore with disposable databases and their matching spools.

## Verification

Run Cargo commands from the repository root with `rust-toolchain.toml`. For integration tests, set `TEST_DATABASE_ADMIN_URL` to an isolated PostgreSQL service and `AI_GATEWAY_TEST_CODEX_PLUGIN` to the library built by `./scripts/prepare-connector-tests.sh`. See [test setup](docs/development/getting-started.md).

```sh
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets --features sqlite-backend
cargo test --locked --workspace --features sqlite-backend
```

- **Forwarding:** Run deterministic transport tests and `./scripts/run-real-upstream-smoke.sh`. The latter makes paid external calls and reads ignored `.env.real-upstream`; obtain authorization and keep credentials out of output. Run `scripts/run-forwarding-perf.sh` only on an explicit performance request.
- **SQLite changes:** Run [system E2E](docs/development/system-e2e.md) on SQLite and PostgreSQL. Backup/restore tests need an offline paired database and spool, not a live service directory.
- **UI changes:** Run `typecheck`, `lint`, `test` and `build` with `pnpm --dir web/console`; add `generate:api:check` for contract changes and `e2e` for browser flows. Build the UI before testing `embedded-console-ui`.
- **Documentation:** Follow [the writing guide](docs/documentation-standard.md); run `git diff --check` and `python3 scripts/check-docs.py`.
- **CI and releases:** See [CI](docs/development/continuous-integration.md) and [release steps](docs/development/releasing.md) before changing workflows or packaging. Keep external Actions SHA-pinned, cache writes out of PRs, and licenses and notices with binary distributions.
