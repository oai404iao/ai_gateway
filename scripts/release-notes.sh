#!/usr/bin/env bash
set -euo pipefail

requested="${1:-}"
version="${requested#v}"
if [[ -z "$version" ]]; then
    echo "usage: $0 <version-or-v-tag>" >&2
    exit 2
fi

awk -v heading="## [$version]" '
    index($0, heading) == 1 { found = 1; next }
    found && /^## \[/ { exit }
    found { print }
' CHANGELOG.md
