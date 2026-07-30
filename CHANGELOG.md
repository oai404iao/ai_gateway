# Changelog

All notable changes to this project are documented in this file.

The format follows Keep a Changelog, and this project uses Semantic
Versioning.

## [Unreleased]

### Fixed

- Restrict Pull Request CI and Security activity to open, update, and reopen
  events so merging and deleting the source branch cannot create a false
  failed `ci-gate`.

## [0.6.2] - 2026-07-30

### Changed

- Parallelize release-specific verification and native image builds, reuse the
  AMD64 container binary for release archives, trust the exact `main`
  `ci-gate`, and move ARM64 cache uploads out of the publishing critical path.
- Upgrade the SHA-pinned Docker login Action from v4.4.0 to v4.6.0 for GHCR
  publishing.

## [0.6.1] - 2026-07-30

### Fixed

- Stabilize passive-health integration tests by keeping refused TCP endpoints
  reserved so concurrently started test servers cannot reuse unavailable
  ports.
- Stabilize Console channel-detail tests by waiting for lazy-loaded forms to
  finish hydrating before assertions and interaction.
- Stabilize durable request-log recovery coverage by waiting for replay and
  settlement to finish before shutting down the restarted worker.

## [0.6.0] - 2026-07-30

### Added

- A statically linked in-process upstream connector registry and the first
  provider implementation, Codex OAuth Connect, with PKCE/token import,
  provider-managed Responses channels, per-credential proxies, serialized
  token refresh, in-place account reauthorization, quota-aware draining, and
  fail-closed Session-affinity credential continuity.
- Administrator Console APIs and a dedicated Codex credential page for
  connecting accounts, importing tokens, editing routing settings, refreshing
  tokens/quota, and reviewing account and rate-limit state.
- Administrator-initiated proxy diagnostics for unsaved proxy drafts,
  including observed egress IP, location, ISP/ASN, proxy and hosting flags,
  latency, and provider rate-limit state.

### Changed

- Identify Codex OAuth authorization, model/quota discovery, and Responses
  forwarding with the pinned `codex_cli_rs/0.146.0` client identity, and
  reorganize credential actions with accessible quota/token refresh controls.
- Harden GitHub development and release automation with a stable path-aware
  `ci-gate`, reusable Rust/Console/Playwright checks, dependency review,
  CodeQL scanning, write-restricted PR caches, protected `main`, and
  worktree-isolated development.

### Fixed

- Report Codex client version `0.146.0` independently from the Gateway package
  version so model discovery does not return an empty catalog for otherwise
  valid OAuth credentials.
- Force identity-encoded Codex SSE responses and classify successful Codex
  responses by the Connector contract, so terminal usage is parsed and
  completed client requests are not misreported as cancelled.
- Allow Console proxy records to persist the already-supported `socks4a` and
  `socks5h` remote-DNS schemes.
- Refetch Console request logs and cost statistics when users intentionally
  submit the same filters more than once.
- Keep existing channel transform templates visible while disabling inactive
  or API-incompatible choices with an explicit reason, and show each
  template's API format in the list.

## [0.5.1] - 2026-07-29

### Added

- Time to first token (TTFT) in Console request-log duration cells, displayed
  above total request duration with accessible labels.
- An apply-ready transform reference template that removes common reverse
  proxy and CDN forwarding metadata before requests reach the upstream.

### Changed

- Reworked the project README into a concise GitHub-facing overview with
  badges, a clearer quick start, improved documentation navigation, and a
  dedicated project logo.

## [0.5.0] - 2026-07-29

### Added

- Request protocol classification on user and administrator request logs,
  distinguishing non-streaming HTTP, SSE, and Responses WebSocket requests.
- Request-log reasoning-token extraction and persistence for Chat Completions
  and Responses, with a compact Console Tokens column that separates uncached
  input, cached input, non-reasoning output, and reasoning tokens.
- Device-aware Console login-session management with current-device markers,
  per-session sign-out, sign-out-all-other-devices, explicit active/expired/
  revoked states, and collapsible session history.
- OpenAI Responses WebSocket proxying on `/v1/responses`, including
  per-message authentication/admission/routing/logging, model and event
  transforms, HTTP/SOCKS proxy support, connection-local
  `previous_response_id` continuity, graceful process draining, and a bounded
  upstream WebSocket pool with Codex-compatible per-message deflate.
- Database-backed Responses WebSocket controls with explicit system, user, and
  channel opt-in; configurable idle-pool capacity and lifetimes; a personal
  settings page; channel capability controls; and current-process pool/session
  metrics on the administrator system-load page.

### Changed

- Unified Console request-log visibility: self-service views remain
  owner-scoped and redact user/channel identity for every role, while the
  administrator Operations view adds user and channel names without showing
  internal IDs. List and detail fields now use the same reduced presentation.

## [0.4.0] - 2026-07-27

### Added

- Self-service Console registration through administrator-managed reusable
  invitation codes with optional usage limits and expiry, adjustable user
  group and initial balance defaults, hash-only storage, and immediate login.
- Administrator-managed user groups with protected default user/admin groups,
  inherited API key policies, and optional per-user policy overrides.
- Atomic batch user updates for status, balance set/increase/decrease, API
  policy overrides, and group membership.
- Confirmed user deletion that anonymizes the account, revokes credentials,
  preserves request/audit ownership, and releases the email for reuse.
- A personal 365-day request-activity contribution calendar with active-day
  and streak summaries for every Console user.
- Periodic day, week, and month spend leaderboards backed by durable snapshots
  and refreshed asynchronously.
- A project-owned gateway mark shared by the Console brand and favicon.

### Changed

- Scheduled channel tests now record token usage and bill the hidden system
  administrator account using configured model prices, advanced billing rules,
  and channel multipliers.
- Request-log tables hide internal channel IDs while preserving them in the
  administrator detail view.
- Moved administrator system-load monitoring from Statistics to a dedicated
  page in the Operations navigation.

## [0.3.1] - 2026-07-25

### Added

- Partial administrator user updates, independent account/balance/status
  controls, and configurable initial balances for invited users.
- Safe invitation reissuance for never-activated users, including recovery
  from historically disabled pending accounts and revocation of older tokens.

### Fixed

- Preserve the `invited` state when administrators edit a pending user's
  profile or balance, so the invitation can still be activated.

## [0.3.0] - 2026-07-24

### Added

- Model-first routing that precompiles model-capable channel candidates and
  distinguishes inaccessible models from configured routes with no healthy
  channels.
- Administrator controls for inspecting and clearing active session-affinity
  cache entries, with audit logging.
- Channel enable, disable, and manual recovery actions with concurrency
  protection.
- Channel-aware request logs and cost statistics, including channel-group
  context, administrator filtering and aggregation, and server-enforced
  redaction for regular users.

### Changed

- Reworked runtime routing around dense channel and route slots, shared
  authorization profiles, precompiled accessible-route bitsets, and sharded
  atomic state.
- Reorganized dense Console settings and resource editors into responsive
  two-column cards while keeping wide model, rule, and JSON editors full width.

## [0.2.0] - 2026-07-24

### Added

- Administrator-managed API host URLs, displayed with copy controls on users'
  API Key pages.
- A public Console Channel status page in the left navigation.

### Changed

- Moved channel status out of Statistics; Statistics now focuses on cost
  analytics and administrator system load.

## [0.1.2] - 2026-07-23

### Changed

- Reworked tagged GHCR publishing to build AMD64 and ARM64 images in parallel
  on native GitHub-hosted runners, then merge their digests into the final
  multi-platform manifest.
- Split Docker build caches by architecture and pinned the architecture-neutral
  Console and `cargo-chef prepare` stages to the native build platform.
- Upgraded release artifact uploads to the Node.js 24-based
  `actions/upload-artifact` v7.

## [0.1.1] - 2026-07-23

### Added

- Per-channel billing multipliers applied to effective request price snapshots
  and settlement.
- Atomic, versioned batch channel updates in the Console API and web UI.
- Draft channel model discovery through the upstream OpenAI-compatible
  `/v1/models` endpoint, with searchable multi-select editing in the Console.

### Changed

- Migrated CI and tagged release automation from Gitea Actions to GitHub
  Actions, GitHub Releases, and GitHub Container Registry.
- Hardened and optimized GitHub Actions with immutable action references,
  least-privilege release jobs, pnpm and Docker caches, workspace-wide Rust
  checks, short-lived release artifacts, image metadata, and public-image
  provenance attestations.
- Added cargo-chef dependency layering to production image builds.
- Licensed the project under `AGPL-3.0-only`, with license metadata and
  required project and third-party notices included in release archives and
  production container images.

### Security

- Reviewed the repository's full Git history with Gitleaks and narrowly
  allowlisted the known-public Ed25519 fixture used only by Console integration
  tests.

## [0.1.0] - 2026-07-22

### Added

- OpenAI-compatible Chat Completions and Responses proxy routes with streaming
  forwarding, model routing, constrained transforms, and upstream
  authentication.
- PostgreSQL-backed control plane, JWT Console API, and embedded React Console
  UI for user and administrator workflows.
- Admission controls, passive health, pre-header failover, scheduled channel
  tests, automatic disable, and process-local session affinity.
- Durable request-log spooling, asynchronous database projection, usage
  extraction, USD settlement, statistics, and system load monitoring.
- Production Docker image, full-stack `docker-compose.prd.yaml`, release
  validation scripts, Gitea Actions CI, release assets, and optional container
  registry publication.
