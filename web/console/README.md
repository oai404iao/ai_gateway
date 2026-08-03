# ai-gateway Console UI

Embedded admin console for `ai-gateway`: a React 19 + TypeScript single-page
app built with Vite, Tailwind CSS v4, and shadcn/ui (Radix). The production
build is embedded into the Rust binary via `rust-embed` (see the
`embedded-console-ui` cargo feature) and served only on the Console listener,
never on the public `/v1/*` data plane. See [`docs/development/console-ui.md`](../../docs/development/console-ui.md)
for the architecture and [`docs/openapi/console-v1.yaml`](../../docs/openapi/console-v1.yaml)
for the API contract this UI consumes.

## Prerequisites

- Node.js 24 and pnpm 11.17.0 (pinned by `packageManager`)
- For component/e2e tests: a POSIX shell; Playwright installs its own Chromium

## Common commands

```bash
pnpm install                # first run only

pnpm dev                    # Vite dev server (HTTPS, https://console.localhost:5173)
                            # Proxy /console/v1 -> http://127.0.0.1:3001 (the Rust
                            # Console listener). Start the gateway separately.

pnpm typecheck              # tsc --noEmit (strict)
pnpm lint                   # oxlint
pnpm test                   # vitest component tests (jsdom + MSW)
pnpm test:watch             # vitest watch mode

pnpm build                  # tsc -b && vite build -> dist/ (embedded by rust-embed)

pnpm generate:api           # regenerate src/api/generated/console-v1.d.ts
pnpm generate:api:check     # CI gate: fails if generated types drifted from spec

pnpm e2e                    # Playwright browser tests (HTTP dev server on :5174)
pnpm e2e:install            # install Playwright Chromium + OS deps (first run)
```

## Layout

- `src/api/` — typed Console client, session store, MSW/test helpers, generated
  OpenAPI types (`generated/console-v1.d.ts` is the single source of truth;
  `types.ts` re-exports it).
- `src/app/` — providers, router, layouts, theme.
- `src/features/` — feature modules (auth, profile, sessions, api-keys,
  owner-scoped request logs and cost statistics, administrator system request
  logs and cost statistics, channel status, spend leaderboard,
  self-registration, user/user-group/registration-code management, system load,
  proxy egress-IP diagnostics, user-group-scoped read-only Codex quota windows,
  administrator Codex OAuth credential/quota management,
  Business workspace-member identity, single/batch credential deletion and
  state changes, Codex credential export/import review with in-page proxy management, and the
  remaining admin control plane).
- `src/components/` — shadcn/ui primitives (`ui/`) and shared app components.
- `src/test/` — vitest setup, MSW server, deterministic fixtures.
- `e2e/` — Playwright browser smoke tests (API mocked at the network layer).

## API contract

The TypeScript types consumed across the app are generated from
`docs/openapi/console-v1.yaml`. Never hand-edit
`src/api/generated/console-v1.d.ts`; change the spec, run `pnpm generate:api`,
and commit both. `pnpm generate:api:check` is the drift gate.

## Testing strategy

- **Component tests** (`*.test.tsx`, vitest + jsdom + MSW): exercise pages and
  flows with deterministic MSW handlers and a per-mount `QueryClient` so cached
  query state never leaks across tests.
- **E2E** (`e2e/`, Playwright + Chromium): runs the real SPA in a browser with
  `/console/v1/*` fulfilled at the network layer, independent of the Rust
  binary or PostgreSQL. Add flows here as the UI grows.

## Licenses

The Console UI is part of `ai-gateway` and is licensed under
`AGPL-3.0-only`; see the repository root `LICENSE`. Most runtime and dev
dependencies use permissive licenses. Two third-party components are worth
calling out:

- `@fontsource-variable/geist` (OFL-1.1) — the Geist font is bundled into
  `dist/` and shipped in the binary. The SIL Open Font License requires the
  license text to accompany redistribution; see `NOTICES.md`.
- `lightningcss` (MPL-2.0) — a build-time CSS tool used by Vite/Tailwind; it is
  not shipped in the binary. MPL-2.0 is weak, file-level copyleft and fine for
  dev tooling.

See `NOTICES.md` for the attribution list and the repository root
`LICENSES/` directory for committed third-party license texts.
