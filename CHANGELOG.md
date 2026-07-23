# Changelog

All notable changes to this project are documented in this file.

The format follows Keep a Changelog, and this project uses Semantic
Versioning.

## [Unreleased]

### Added

- Per-channel billing multipliers applied to effective request price snapshots
  and settlement.
- Atomic, versioned batch channel updates in the Console API and web UI.

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
