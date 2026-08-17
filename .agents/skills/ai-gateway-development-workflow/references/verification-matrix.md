# Verification matrix

Run commands from the repository root. Combine every row that matches the
change. Narrow tests are useful while iterating but do not replace the listed
completion gate.

## Documentation and agent instructions only

```bash
git diff --check
python3 scripts/check-docs.py
```

- Verify command examples against the current scripts and package manifests.
- Confirm links and referenced paths exist.
- `.github/workflows/ci.yml` selects the documentation job for Markdown-only
  changes and still emits the required `ci-gate`.

## Rust backend or shared workspace

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

PostgreSQL-backed tests require the development database:

```bash
docker compose up -d
```

## Console frontend

```bash
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build
```

The five documented shadcn Fast Refresh warnings are accepted. New lint
warnings or any lint error are not.

Run browser smoke tests when navigation, authentication, forms, overlays,
responsive behavior, or a critical user flow changes:

```bash
pnpm --dir web/console e2e
```

## Console API contract

Edit `docs/openapi/console-v1.yaml` first, then:

```bash
pnpm --dir web/console generate:api
pnpm --dir web/console generate:api:check
cargo test --locked --test console_spec_integration
```

Commit the OpenAPI specification and generated TypeScript declaration
together. Never hand-edit the generated declaration.

## Embedded Console UI

Build the frontend before exercising Rust embedding:

```bash
pnpm --dir web/console build
cargo clippy --locked --all-targets --features embedded-console-ui
cargo test --locked --features embedded-console-ui --lib console_ui
```

## Database schema or persistence

```bash
docker compose up -d
cargo test --locked --workspace
```

- Add an ordered migration; never modify a deployed migration in place.
- Test repository ownership and authorization rules in SQL, not only in
  handlers.
- For SQLite schema, storage-adapter, or runtime-snapshot repository changes,
  also run:

  ```bash
  cargo clippy --locked --all-targets --features sqlite-backend
  cargo test --locked --features sqlite-backend --lib
  cargo test --locked --features sqlite-backend --test sqlite_schema_integration
  cargo test --locked --features sqlite-backend --test sqlite_runtime_repository_integration
  ```

  SQLite currently has an independent baseline and runtime-snapshot reader,
  but process configuration and complete repository dispatch remain
  PostgreSQL-only.

## Forwarding path

Run the normal Rust gate and relevant proxy/streaming tests.

Before completion, obtain explicit user authorization for the paid
real-upstream smoke test, then run:

```bash
./scripts/run-real-upstream-smoke.sh
```

If authorization or credentials are unavailable, keep the PR draft or report
the forwarding change as blocked; do not claim completion.

## Performance tooling

The lightweight package checks are safe:

```bash
cargo test --locked --package ai-gateway-perf
cargo clippy --locked --package ai-gateway-perf --all-targets
```

Never run `scripts/run-forwarding-perf.sh` unless the user explicitly requests
a performance run.

## Docker or deployment

```bash
docker compose -f docker-compose.prd.yaml config --quiet
docker build -t ai-gateway:local .
docker run --rm ai-gateway:local --version
```

Also validate secret paths, privilege drop, embedded assets, license labels,
and redistribution notices when those areas change.

## Release preparation

```bash
./scripts/check-release-version.sh <version>
./scripts/verify-release.sh <version>
```

The verification script is the release gate and includes Rust, Console,
embedded UI, Compose, Docker, version, and redistribution checks.

## Always before handing off

```bash
git diff --check
git status --short
```

List any skipped command with the reason. Do not silently downgrade a required
gate.
