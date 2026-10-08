#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if (( $# > 1 )); then
  printf 'Usage: %s [explicit-local-plugin-repository]\n' "$0" >&2
  exit 2
fi

umask 077
scratch_root="$HOME/.local/state/agents/tmp"
mkdir -p -- "$scratch_root"
task_dir="$(mktemp -d "$scratch_root/connector-test-build.XXXXXXXX")"

if (( $# == 1 )); then
  source_root="$(cd "$1" && pwd)"
else
  lock="$repo_root/tests/fixtures/codex-plugin.json"
  revision="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["revision"])' "$lock")"
  if [[ ! "$revision" =~ ^[0-9a-f]{40}$ ]]; then
    printf 'The test plugin revision must be a full commit SHA.\n' >&2
    exit 2
  fi
  source_root="$task_dir/source"
  git init -q "$source_root"
  git -C "$source_root" remote add origin https://github.com/oai404iao/ai-gateway-connectors.git
  git -C "$source_root" fetch -q --depth=1 origin "$revision"
  git -C "$source_root" checkout -q --detach FETCH_HEAD
  test "$(git -C "$source_root" rev-parse HEAD)" = "$revision"
fi

(
  cd "$source_root"
  CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 TMPDIR="$task_dir" \
    cargo build --locked --package ai-gateway-connector-codex
) >&2

library="$source_root/target/debug/libai_gateway_connector_codex.so"
hash="$(sha256sum "$library" | cut -d ' ' -f 1)"
destination="$repo_root/target/connector-tests/$hash"
mkdir -p -- "$destination"
if [[ ! -f "$destination/libai_gateway_connector_codex.so" ]]; then
  install -m 0444 "$library" "$destination/libai_gateway_connector_codex.so"
fi
test "$(sha256sum "$destination/libai_gateway_connector_codex.so" | cut -d ' ' -f 1)" = "$hash"
for mode in 0 1 2; do
  cc -shared -fPIC -Wall -Wextra -Werror "-DFIXTURE_BODY_MODE=$mode" \
    "$repo_root/crates/connector-sdk/tests/fixture.c" -o "$task_dir/fixture-$mode.so"
  install -m 0444 "$task_dir/fixture-$mode.so" "$destination/fixture-$mode.so"
done
cc -shared -fPIC -Wall -Wextra -Werror -DFIXTURE_EMPTY_COMMANDS \
  "$repo_root/crates/connector-sdk/tests/fixture.c" -o "$task_dir/fixture-empty.so"
install -m 0444 "$task_dir/fixture-empty.so" "$destination/fixture-empty.so"
printf '%s\n' "$destination/libai_gateway_connector_codex.so"
