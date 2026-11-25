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

- Node.js 24 (declared in `engines`) and pnpm 12.10.1 (pinned by
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

## Navigate the Console

After sign-in, `/` shows the current account’s overview for both users and
administrators. `/statistics` is the personal usage report. Use the desktop
sidebar or the mobile dock to switch workspaces. **Find page** (`Ctrl+K` or
`⌘K`) and the dock’s **More** button search the pages available to the account. The
avatar menu contains account details, preferences, sessions and sign-out.

Administrators can enter the management area without changing accounts. Its
local navigation appears only on `/admin` routes. Deep links open the active
branch of the management tree; on mobile, tap the current page above the
content to browse. Personal reports and logs
remain personal; `/admin/cost-statistics` and `/admin/request-logs` are the
separate cross-user views. The workspace reads from the authenticated Console
API, including when served by Vite. Fixtures and network mocks belong to tests.

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

Key and Policy forms save group and individual channel selections separately.
A selected group follows its current membership; a directly selected channel
stays selected if it moves. Sharing an upstream credential does not share a
grant. See the [authorization and topology contract](../../docs/development/upstream-identity-capabilities.md).

The management area provides **Upstream accesses**, **Upstream credentials**,
**Channel configuration**, and **Model configuration**.
Channel configuration combines group management, logical channels grouped by
their group, and capabilities scoped to the selected channel.

`/admin/models` has a searchable client-model directory and an embedded
operation-route editor. Wide, tall windows scroll the two panes independently;
single-pane layouts return to the original list position. Select a model, add an unused operation,
then configure explicit `(capability, upstream model)` candidates and priority
tiers. The single-operation editor creates a missing model routing profile before
saving the rule; these are separate commands, and retries reuse the profile.
Existing rules retain ETag updates and draft navigation protection. Price sync
is a separate tab; model metadata, copying, and pricing retain dedicated editors.
For multiple providers or operations, use **Onboard model** or the selected model's
**Bulk configure routes** action. These submit one reviewed transaction. For catalog-only
changes, use **Append upstream model** in the capability list.
See the [Console interaction standard](../../docs/development/console-interaction-standard.md) and
browser coverage in `e2e/model-onboarding.spec.ts`, `e2e/ux-model-workspace.spec.ts`
and `e2e/routing-topology.spec.ts`.

## Plugin administration

Administrator **Plugins** pages (`/admin/plugins` and `/admin/plugins/:id`)
manage native package installation, directory discovery, version activation,
disablement, inactive artifact deletion, and plugin-defined settings.
The Console renders the settings fields declared by each plugin. Configure Codex
on its plugin detail page; manage its OAuth accounts under **Upstream credentials**.

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
Base UI. The `@coss` registry supplements these wrappers with [Frame](https://coss.com/ui/docs/components/frame) for grouped panels. Tables and charts keep using the shared Table and Recharts-backed Chart components. Custom triggers use `render`, not Radix `asChild`.

- Follow [`design.md`](../../design.md) for visual rules: centralized CSS variables,
  Lucide icons, and Motion (`motion/react`) for application animations. Keep
  reduced-motion behavior; Nova/Base UI primitive transitions remain component-owned.
- Use Zustand for shared client state (memory-only session, theme and locale).
  Keep server state in TanStack Query, forms in React Hook Form, and local state local.
- Follow the [interaction standard](../../docs/development/console-interaction-standard.md)
  for parent links, URL list state, detail layouts, unsaved drafts and labels.
  Navigation uses links; mutations use buttons. Do not duplicate Back controls.
- Seed every Select-backed React Hook Form field in `defaultValues` so `reset()`
  values participate in validation. Show field errors and retain failed drafts.
- Keep the query cache inside the mounted session provider. Changing account or privilege scope starts a fresh cache; refreshing the bearer for the same scope does not.
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
