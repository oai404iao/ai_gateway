#!/usr/bin/env bash
set -euo pipefail

# Classify newline-delimited repository paths for the path-aware CI workflow.
# Unknown files deliberately select every expensive gate so new repository
# areas cannot silently bypass verification.
# `image` identifies image recipes/cache inputs; main CI separately includes
# ordinary Rust and Console changes in its image build.

docs=false
rust=false
console=false
image=false
seen=false

while IFS= read -r path; do
    [[ -z "$path" ]] && continue
    seen=true

    case "$path" in
        .gitignore)
            docs=true
            ;;
        .github/* | scripts/*)
            docs=true
            rust=true
            console=true
            image=true
            ;;
        docs/openapi/*)
            docs=true
            rust=true
            console=true
            image=true
            ;;
        src/* | tests/* | migrations/* | tools/*)
            rust=true
            ;;
        Cargo.toml | Cargo.lock | rust-toolchain.toml)
            rust=true
            image=true
            ;;
        web/console/package.json | web/console/pnpm-lock.yaml | web/console/pnpm-workspace.yaml)
            console=true
            image=true
            ;;
        web/console/*)
            console=true
            ;;
        Dockerfile | .dockerignore | docker-compose*.yml | config.example.toml | deploy/* | LICENSE | LICENSES/*)
            image=true
            ;;
        *.md | docs/* | .agents/*)
            docs=true
            ;;
        *)
            docs=true
            rust=true
            console=true
            image=true
            ;;
    esac
done

if [[ "$seen" == false ]]; then
    docs=true
    rust=true
    console=true
    image=true
fi

printf 'docs=%s\n' "$docs"
printf 'rust=%s\n' "$rust"
printf 'console=%s\n' "$console"
printf 'image=%s\n' "$image"
