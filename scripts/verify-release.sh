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
cargo clippy --locked --all-targets --features mcp-server
cargo test --locked --workspace
cargo test --locked --features mcp-server --lib
cargo test --locked --features mcp-server --test mcp_integration

pnpm --dir web/console install --frozen-lockfile
pnpm --dir web/console generate:api:check
pnpm --dir web/console typecheck
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build

cargo clippy --locked --all-targets --features embedded-console-ui,mcp-server
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
docker image inspect \
    --format '{{ index .Config.Labels "org.opencontainers.image.licenses" }}' \
    "$image" \
    | grep -Fx "AGPL-3.0-only"
docker run --rm --entrypoint /bin/sh "$image" -ec '
    test -s /usr/share/doc/ai-gateway/LICENSE
    test -s /usr/share/doc/ai-gateway/THIRD_PARTY_NOTICES.md
    test -s /usr/share/doc/ai-gateway/LICENSES/cargo/axum_0.8.9/LICENSE
    test -s /usr/share/doc/ai-gateway/LICENSES/cargo/rmcp_3.1.1/UPSTREAM_Cargo.toml
    test -s /usr/share/doc/ai-gateway/LICENSES/npm/fontsource-variable_geist_5.3.0/LICENSE
'

echo "release verification passed: v${version}"
