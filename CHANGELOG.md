# Changelog

All notable changes to this project are documented in this file.

The format follows Keep a Changelog, and this project uses Semantic
Versioning.

## [Unreleased]

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
