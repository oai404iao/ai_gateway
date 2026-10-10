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
task_dir="$(mktemp -d "$scratch_root/usage-parser-test-build.XXXXXXXX")"
target="$repo_root/target/usage-parser-tests-build"
(
  cd "$repo_root"
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR="$task_dir" \
    cargo build --locked --package ai-gateway-connector-sdk --example usage_parser \
      --target-dir "$target"
) >&2
library="$target/debug/examples/libusage_parser.so"
hash="$(sha256sum "$library" | cut -d ' ' -f 1)"
destination="$repo_root/target/usage-parser-tests/$hash"
mkdir -p -- "$destination"
if [[ ! -f "$destination/libusage_parser.so" ]]; then
  install -m 0444 "$library" "$destination/libusage_parser.so"
fi
test "$(sha256sum "$destination/libusage_parser.so" | cut -d ' ' -f 1)" = "$hash"
python3 "$repo_root/scripts/package-test-plugin.py" \
  "$destination/libusage_parser.so" "$repo_root" "$destination" example-usage-parser
printf '%s\n' "$destination/libusage_parser.so"
