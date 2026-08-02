#!/usr/bin/env bash
# Runs the ignored, paid real-upstream smoke test. This script is intentionally
# the only repository tooling that loads a local .env file.
set -euo pipefail
set +x

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

mode="${1:---all}"
if (( $# > 1 )); then
  printf 'Usage: %s [--all|--images-edit-only]\n' "$0" >&2
  exit 2
fi
case "$mode" in
  --all | --images-edit-only) ;;
  *)
    printf 'Usage: %s [--all|--images-edit-only]\n' "$0" >&2
    exit 2
    ;;
esac

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

responses_profile="${REAL_UPSTREAM_RESPONSES_PROFILE:-openai_compatible}"
case "$responses_profile" in
  openai_compatible | codex_oauth) ;;
  *)
    printf '%s\n' \
      'REAL_UPSTREAM_RESPONSES_PROFILE must be openai_compatible or codex_oauth.' >&2
    exit 2
    ;;
esac

websocket_base_set=0
websocket_key_set=0
[[ -n "${REAL_UPSTREAM_WEBSOCKET_BASE_URL:-}" ]] && websocket_base_set=1
[[ -n "${REAL_UPSTREAM_WEBSOCKET_API_KEY:-}" ]] && websocket_key_set=1
if (( websocket_base_set != websocket_key_set )); then
  printf '%s\n' \
    'REAL_UPSTREAM_WEBSOCKET_BASE_URL and REAL_UPSTREAM_WEBSOCKET_API_KEY must be set together.' >&2
  exit 2
fi

images_settings=(
  REAL_UPSTREAM_IMAGES_BASE_URL
  REAL_UPSTREAM_IMAGES_API_KEY
  REAL_UPSTREAM_IMAGES_MODEL
)
images_configured=0
for name in "${images_settings[@]}"; do
  [[ -n "${!name:-}" ]] && ((images_configured += 1))
done
if (( images_configured != 0 && images_configured != ${#images_settings[@]} )); then
  printf '%s\n' \
    'REAL_UPSTREAM_IMAGES_BASE_URL, REAL_UPSTREAM_IMAGES_API_KEY, and REAL_UPSTREAM_IMAGES_MODEL must be set together.' >&2
  exit 2
fi

export RUN_REAL_UPSTREAM_SMOKE=1
cargo_args=(test --test real_upstream)
test_args=(--ignored --test-threads=1 --show-output)
case "$mode" in
  --all)
    if (( images_configured == 0 )); then
      test_args+=(--skip suite::images)
    fi
    ;;
  --images-edit-only)
    if (( images_configured == 0 )); then
      printf '%s\n' \
        '--images-edit-only requires all REAL_UPSTREAM_IMAGES_* settings.' >&2
      exit 2
    fi
    cargo_args+=(suite::images::captures_images_edit_usage_from_a_real_upstream)
    test_args+=(--exact)
    ;;
esac
cargo "${cargo_args[@]}" -- "${test_args[@]}"
