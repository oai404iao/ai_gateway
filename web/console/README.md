# ai-gateway Console UI

> Status: Current frontend development and build guide.

The Console is a React 19 + TypeScript SPA for users and administrators. Vite
serves it during development; the production build is embedded in the Rust
binary and served only on the Console listener. There is no production Node
service, SSR, or UI route on the public `/v1/*` listener.

## On this page

- [Run the development UI](#run-the-development-ui)
- [Build embedded assets](#build-embedded-assets)
- [Change the API contract](#api-contract)
- [Work on pages and forms](#page-and-form-conventions)
- [Run checks](#testing-strategy)
- [Find source files](#layout)

## Prerequisites

- Node.js 24 (declared in `engines`) and pnpm 11.17.0 (pinned by
  `packageManager`)
- A POSIX shell for test scripts; Playwright Chromium for browser tests
- For live development: a running Gateway with Console/JWT configured and a
  login account. Mocked component/browser tests do not need a Gateway or database.

## Run the development UI

Run frontend commands from `web/console/`. From the repository root, use
`pnpm --dir web/console <command>` instead.

```bash
pnpm install --frozen-lockfile
pnpm dev
```

Open `https://console.localhost:5173`. Vite proxies `/console/v1/*` to the Rust
Console listener at `http://127.0.0.1:3001`; start the Gateway separately from the
repository root with `cargo run`. `[console].ui_enabled` does not control Vite.

## Build embedded assets

```bash
pnpm build
```

Then run the Rust build from the repository root:

```bash
cargo build --release --features embedded-console-ui
```

The frontend command writes `dist/`; the Rust command embeds it using
`rust-embed`. Set `[console].ui_enabled = true` to serve it. Startup rejects
that setting if the Cargo feature is absent. Debug builds read `dist/` from
disk at runtime; release builds embed the assets. Explicit Console API routes
take precedence over the GET/HEAD SPA fallback, including JSON 404s for unknown
`/console/v1/*` paths.

See the [UI architecture](../../docs/development/console-ui.md) for cache and
security headers, listener tests, and embedding details.

## Layout

- `src/api/` — typed Console client, session store, MSW/test helpers, generated
  OpenAPI types (`generated/console-v1.d.ts` is generated from the authoritative
  repository spec; `types.ts` re-exports it).
- `src/app/` — providers, router, layouts, theme.
- `src/features/` — auth, self-registration, profile, sessions, personal API
  keys/logs/cost statistics, and administrator control-plane pages. Admin
  modules include users/groups/registration codes, model pricing and routing,
  status/load/leaderboards, proxy egress diagnostics, plugins, categorized
  settings, and reusable upstream credentials. Codex pages cover workspace/member
  identity, quota windows, single/batch maintenance, and export/import review
  with in-page proxy management; personal quota views are group-scoped and read-only.
- `src/components/` — shadcn/ui primitives (`ui/`) and shared app components.
- `src/test/` — vitest setup, MSW server, deterministic fixtures.
- `e2e/` — Playwright browser smoke tests (API mocked at the network layer).

## Model configuration

Key/Policy grants target fixed logical channels. Credentials are independent
of groups; each channel explicitly binds its access and credential. Adding a
capability shares the channel's grant, but adding a channel to a group does not
silently grant access. Deployment requires the coordinated migrations in the
[cutover record](../../docs/development/upstream-identity-capabilities.md).

The sidebar groups configuration into **Upstream accesses**, **Upstream
credentials**, **Channel configuration**, and **Model configuration**.
Channel configuration combines group management, logical channels grouped by
their group, and capabilities scoped to the selected channel.

`/admin/models` lists client models on the left and embeds the existing
operation-route editor on the right. Select a model, add an unused operation,
then configure explicit `(capability, upstream model)` candidates and priority
tiers. The first operation save creates a missing model routing profile before
saving the rule; these are separate commands, and retries reuse the profile.
Existing rules retain ETag updates and draft navigation protection. Price sync
is a separate tab; model metadata, copying, and pricing retain dedicated editors.
Regression coverage lives beside these pages and in `e2e/routing-topology.spec.ts`.

## Plugin administration

Administrator **Plugins** pages (`/admin/plugins` and `/admin/plugins/:id`)
manage native package installation, directory discovery, version activation,
disablement, inactive artifact deletion, and plugin-defined settings.
Settings forms render bounded scalar descriptors; no provider-specific fields,
plugin JavaScript, or credential storage are embedded in the form renderer.
System settings no longer contain a Codex category.

Every plugin mutation prompts for administrator password verification. The
single-use authorization token stays in the current operation's memory, never
browser storage. Install uploads use raw `application/octet-stream` tar.gz
bytes. Install/discovery jobs are polled and their IDs remain in the page URL
so progress can be reopened after refresh. Version and settings updates use
server-issued ETags; conflicts reload the current resource.

## API contract

```bash
pnpm generate:api
pnpm generate:api:check
```

The TypeScript types consumed across the app are generated from
`docs/openapi/console-v1.yaml`. Never hand-edit
`src/api/generated/console-v1.d.ts`; change the spec, run `pnpm generate:api`,
and commit both. `pnpm generate:api:check` is the drift gate.

## Page and form conventions

Use Tailwind CSS v4 and the checked-in shadcn/ui `base-nova` wrappers backed by
Base UI. Custom triggers use `render`, not Radix `asChild`.

- Follow the [interaction standard](../../docs/development/console-interaction-standard.md)
  for parent links, URL list state, detail layouts, unsaved drafts, labels and
  redirects. Navigation uses links; mutations use buttons. Do not duplicate Back controls.
- Seed every Select-backed React Hook Form field in `defaultValues` so `reset()`
  values participate in validation. Show field errors and retain failed drafts.
- Create a `QueryClient` per provider mount, not as a module singleton.
- Keep access tokens in memory; refresh tokens use HttpOnly Secure cookies.
  Frontend guards are UX, never the authorization boundary.
- Preserve GET ETags and send `If-Match` for versioned mutations. A 409 requires
  reloading the current resource, not overwriting it silently.

## Testing strategy

```bash
pnpm generate:api:check
pnpm typecheck
pnpm lint
pnpm test
pnpm build
pnpm e2e:install             # first run: Chromium and OS dependencies
pnpm e2e
```

`typecheck` runs `tsc -b --noEmit`; lint uses oxlint, not ESLint. The five known
shadcn Fast Refresh warnings are allowed; new warnings/errors are not. Use
`pnpm test:watch` for component-test iteration.

- **Component tests** (`*.test.tsx`, vitest + jsdom + MSW): exercise pages and
  flows with deterministic MSW handlers and a per-mount `QueryClient` so cached
  query state never leaks across tests.
- **E2E** (`e2e/`, Playwright + Chromium): runs the real SPA in a browser with
  `/console/v1/*` fulfilled at the network layer, independent of the Rust
  binary or PostgreSQL. The test server uses HTTP on `127.0.0.1:5174`, not the
  HTTPS development server. Add flows here as the UI grows.
- **System E2E** (repository-root `e2e/`): real embedded Gateway, database,
  browser and fixed CLIs. Use the [system guide](../../docs/development/system-e2e.md)
  when real persistence, cookie rotation or settlement must be verified.

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
