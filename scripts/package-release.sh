#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

requested="${1:-}"
version="${requested#v}"
output_dir="${2:-target/release-package}"
binary="${RELEASE_BINARY:-target/release/ai-gateway}"

if [[ -z "$version" ]]; then
    echo "usage: $0 <version-or-v-tag> [output-directory]" >&2
    exit 2
fi

"$repo_root/scripts/check-release-version.sh" "$version"
if [[ ! -x "$binary" ]]; then
    echo "missing release binary: $binary" >&2
    exit 1
fi

license_materials="${RELEASE_LICENSE_MATERIALS:-target/third-party-licenses}"
if [[ -z "${RELEASE_LICENSE_MATERIALS:-}" ]]; then
    python3 "$repo_root/scripts/generate-third-party-notices.py" \
        --output "$license_materials"
fi
if [[ ! -s "$license_materials/THIRD_PARTY_NOTICES.md" ]] \
    || [[ ! -d "$license_materials/LICENSES" ]]; then
    echo "invalid release license materials: $license_materials" >&2
    exit 1
fi

target="${RELEASE_TARGET:-$(rustc -vV | awk '/^host:/ { print $2 }')}"
if [[ ! "$target" =~ ^[A-Za-z0-9._-]+$ ]]; then
    echo "invalid release target: $target" >&2
    exit 1
fi
archive_base="ai-gateway-v${version}-${target}"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

install -d "$stage/$archive_base"
install -m 0755 "$binary" "$stage/$archive_base/ai-gateway"
install -m 0644 config.example.toml "$stage/$archive_base/config.example.toml"
install -m 0644 deploy/compose/config.example.toml \
    "$stage/$archive_base/config.container.example.toml"
install -m 0644 deploy/compose/env.example \
    "$stage/$archive_base/compose.env.example"
install -m 0644 docker-compose.prd.yaml \
    "$stage/$archive_base/docker-compose.prd.yaml"
install -m 0644 \
    README.md \
    README.zh-CN.md \
    CHANGELOG.md \
    LICENSE \
    "$stage/$archive_base/"
install -m 0644 \
    "$license_materials/THIRD_PARTY_NOTICES.md" \
    "$stage/$archive_base/"
cp -a "$license_materials/LICENSES" "$stage/$archive_base/LICENSES"
cp -a docs "$stage/$archive_base/docs"

mkdir -p "$output_dir"
archive="$output_dir/${archive_base}.tar.gz"
source_date_epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}"
tar \
    --sort=name \
    --mtime="@${source_date_epoch}" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    -C "$stage" \
    -czf "$archive" \
    "$archive_base"

(
    cd "$output_dir"
    sha256sum "$(basename "$archive")" > SHA256SUMS
)

printf '%s\n' "$archive" "$output_dir/SHA256SUMS"
