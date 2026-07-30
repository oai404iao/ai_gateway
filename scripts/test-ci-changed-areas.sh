#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
classifier="$repo_root/scripts/ci-changed-areas.sh"

docs_only=$'docs=true\nrust=false\nconsole=false\nimage=false'
rust_and_image=$'docs=false\nrust=true\nconsole=false\nimage=true'
console_and_image=$'docs=false\nrust=false\nconsole=true\nimage=true'
all_areas=$'docs=true\nrust=true\nconsole=true\nimage=true'

assert_areas() {
    local name="$1"
    local expected="$2"
    local actual
    shift 2

    if (($# == 0)); then
        actual="$("$classifier" </dev/null)"
    else
        actual="$(printf '%s\n' "$@" | "$classifier")"
    fi

    if [[ "$actual" != "$expected" ]]; then
        printf 'case %q failed\nexpected:\n%s\nactual:\n%s\n' \
            "$name" "$expected" "$actual" >&2
        return 1
    fi
}

assert_areas \
    "agent documentation and gitignore" \
    "$docs_only" \
    ".agents/skills/example/SKILL.md" \
    ".gitignore"
assert_areas "Rust source" "$rust_and_image" "src/main.rs"
assert_areas "Console source" "$console_and_image" "web/console/src/main.tsx"
assert_areas "OpenAPI contract" "$all_areas" "docs/openapi/console-v1.yaml"
assert_areas "workflow" "$all_areas" ".github/workflows/ci.yml"
assert_areas "unknown path" "$all_areas" "new-area/file.txt"
assert_areas "empty change set" "$all_areas"

echo "ci changed-area classification tests passed"
