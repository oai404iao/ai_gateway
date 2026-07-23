#!/usr/bin/env bash
set -euo pipefail

tag="${1:-}"
notes_file="${2:-}"
shift 2 || true

if [[ -z "$tag" || ! -f "$notes_file" || "$#" -eq 0 ]]; then
    echo "usage: $0 <tag> <notes-file> <asset>..." >&2
    exit 2
fi
if ! command -v gh >/dev/null 2>&1; then
    echo "GitHub CLI is required: gh" >&2
    exit 1
fi

for asset in "$@"; do
    if [[ ! -f "$asset" ]]; then
        echo "missing release asset: $asset" >&2
        exit 1
    fi
done

repo="${GH_REPO:-${GITHUB_REPOSITORY:-}}"
repo_args=()
if [[ -n "$repo" ]]; then
    repo_args=(--repo "$repo")
fi

if gh release view "$tag" "${repo_args[@]}" >/dev/null 2>&1; then
    echo "GitHub release already exists and will not be overwritten: $tag" >&2
    exit 1
fi

version="${tag#v}"
release_args=(
    "$tag"
    "${repo_args[@]}"
    --verify-tag
    --title "ai-gateway ${version}"
    --notes-file "$notes_file"
)
if [[ "$version" == *-* ]]; then
    release_args+=(--prerelease --latest=false)
fi

gh release create "${release_args[@]}" "$@"
echo "published GitHub release: $tag"
