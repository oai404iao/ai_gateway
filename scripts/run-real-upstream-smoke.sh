#!/usr/bin/env bash
# Runs the ignored, paid real-upstream smoke test. This script is intentionally
# the only repository tooling that loads a local .env file.
set -euo pipefail
set +x

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

env_file="${REAL_UPSTREAM_ENV_FILE:-.env.real-upstream}"
if [[ -f "$env_file" ]]; then
  # This file is developer-controlled shell assignment syntax. It must remain
  # local and ignored; never use an untrusted file path here.
  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
fi

required=(
  REAL_UPSTREAM_BASE_URL
  REAL_UPSTREAM_API_KEY
  REAL_UPSTREAM_CHAT_COMPLETIONS_MODEL
  REAL_UPSTREAM_RESPONSES_MODEL
)
for name in "${required[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf 'Missing required real-upstream smoke setting: %s\n' "$name" >&2
    printf 'Copy .env.real-upstream.example to .env.real-upstream and fill it locally.\n' >&2
    exit 2
  fi
done

codex_bin="${REAL_UPSTREAM_CODEX_BIN:-codex}"
if ! command -v "$codex_bin" >/dev/null 2>&1; then
  printf 'Codex CLI for the real-upstream WebSocket smoke was not found: %s\n' "$codex_bin" >&2
  printf 'Set REAL_UPSTREAM_CODEX_BIN to an installed codex command or absolute path.\n' >&2
  exit 2
fi
export REAL_UPSTREAM_CODEX_BIN="$codex_bin"

export RUN_REAL_UPSTREAM_SMOKE=1
cargo test --test real_upstream -- --ignored --test-threads=1
