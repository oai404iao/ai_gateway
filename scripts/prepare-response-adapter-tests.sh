#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if (( $# != 0 )); then
  printf 'Usage: %s\n' "$0" >&2
  exit 2
fi
umask 077
scratch_root="$HOME/.local/state/agents/tmp"
mkdir -p -- "$scratch_root"
task_dir="$(mktemp -d "$scratch_root/response-adapter-test-build.XXXXXXXX")"
target="$repo_root/target/response-adapter-tests-build"
(
  cd "$repo_root"
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR="$task_dir" \
    cargo build --locked --package ai-gateway-connector-sdk --example response_adapter \
      --target-dir "$target"
) >&2
library="$target/debug/examples/libresponse_adapter.so"
hash="$(sha256sum "$library" | cut -d ' ' -f 1)"
destination="$repo_root/target/response-adapter-tests/$hash"
mkdir -p -- "$destination"
if [[ ! -f "$destination/libresponse_adapter.so" ]]; then
  install -m 0444 "$library" "$destination/libresponse_adapter.so"
fi
test "$(sha256sum "$destination/libresponse_adapter.so" | cut -d ' ' -f 1)" = "$hash"
python3 "$repo_root/scripts/package-test-plugin.py" \
  "$destination/libresponse_adapter.so" "$repo_root" "$destination" example-response-adapter
printf '%s\n' "$destination/libresponse_adapter.so"
