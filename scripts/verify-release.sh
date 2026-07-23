#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

requested="${1:-}"
version="${requested#v}"
if [[ -z "$version" ]]; then
    echo "usage: $0 <version-or-v-tag>" >&2
    exit 2
fi
"$repo_root/scripts/check-release-version.sh" "$version"

cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace

pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console generate:api:check
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build

cargo clippy --locked --all-targets --features embedded-console-ui
cargo test --locked --features embedded-console-ui --lib console_ui

docker compose -f docker-compose.prd.yaml config --quiet
image="ai-gateway:release-check-${version}"
docker build \
    --build-arg "VERSION=${version}" \
    --build-arg "REVISION=$(git rev-parse HEAD)" \
    --build-arg "SOURCE_URL=$(git config --get remote.origin.url)" \
    --tag "$image" \
    .
docker run --rm "$image" --version | grep -Fx "ai-gateway ${version}"

echo "release verification passed: v${version}"
