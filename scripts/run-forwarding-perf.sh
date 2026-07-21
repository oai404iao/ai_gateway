#!/usr/bin/env bash
# Manually builds and runs the isolated end-to-end forwarding performance
# harness. This script is intentionally not called by cargo test or CI.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo build --release --locked \
  --package ai-gateway \
  --package ai-gateway-perf

exec "$repo_root/target/release/ai-gateway-perf" run "$@"
