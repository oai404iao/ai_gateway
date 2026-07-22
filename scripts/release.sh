#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

requested="${1:-}"
version="${requested#v}"
mode="${2:-}"
if [[ -z "$version" || ( -n "$mode" && "$mode" != "--push" ) ]]; then
    echo "usage: $0 <version> [--push]" >&2
    exit 2
fi

if [[ "$(git branch --show-current)" != "main" ]]; then
    echo "releases must be created from main" >&2
    exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
    echo "working tree must be clean before release" >&2
    exit 1
fi

git fetch origin main --tags
if ! git merge-base --is-ancestor origin/main HEAD; then
    echo "local main does not contain origin/main" >&2
    exit 1
fi

tag="v${version}"
if git rev-parse --verify --quiet "refs/tags/${tag}" >/dev/null \
    || git ls-remote --exit-code --tags origin "refs/tags/${tag}" >/dev/null 2>&1; then
    echo "release tag already exists: $tag" >&2
    exit 1
fi

"$repo_root/scripts/verify-release.sh" "$version"
if [[ -n "$(git status --porcelain)" ]]; then
    echo "release verification changed the working tree" >&2
    exit 1
fi
git tag --annotate "$tag" --message "ai-gateway ${version}"

if [[ "$mode" == "--push" ]]; then
    git push --atomic origin main "$tag"
    echo "published release tag: $tag"
else
    echo "created local release tag: $tag"
    echo "publish with: git push --atomic origin main $tag"
fi
