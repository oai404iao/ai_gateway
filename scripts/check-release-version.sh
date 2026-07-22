#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

requested="${1:-}"
if [[ -z "$requested" ]]; then
    requested="$(git describe --tags --exact-match 2>/dev/null || true)"
fi
if [[ -z "$requested" ]]; then
    echo "usage: $0 <version-or-v-tag>" >&2
    exit 2
fi

tag="${requested#refs/tags/}"
version="${tag#v}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
    echo "invalid semantic version: $requested" >&2
    exit 1
fi

python3 - "$version" <<'PY'
import json
import pathlib
import re
import sys
import tomllib

version = sys.argv[1]
root = pathlib.Path(".")

with (root / "Cargo.toml").open("rb") as handle:
    cargo = tomllib.load(handle)
with (root / "Cargo.lock").open("rb") as handle:
    cargo_lock = tomllib.load(handle)
with (root / "tools/forwarding-perf/Cargo.toml").open("rb") as handle:
    perf = tomllib.load(handle)
with (root / "web/console/package.json").open(encoding="utf-8") as handle:
    console = json.load(handle)

checks = {
    "Cargo.toml package.version": cargo["package"]["version"],
    "tools/forwarding-perf/Cargo.toml package.version": perf["package"]["version"],
    "web/console/package.json version": console["version"],
}

lock_versions = [
    package["version"]
    for package in cargo_lock["package"]
    if package["name"] == "ai-gateway"
]
if len(lock_versions) != 1:
    raise SystemExit("Cargo.lock must contain exactly one ai-gateway package")
checks["Cargo.lock ai-gateway version"] = lock_versions[0]

compose = (root / "docker-compose.prd.yaml").read_text(encoding="utf-8")
match = re.search(r"AI_GATEWAY_VERSION:-([^}]+)", compose)
if not match:
    raise SystemExit("docker-compose.prd.yaml has no AI_GATEWAY_VERSION default")
checks["docker-compose.prd.yaml version"] = match.group(1)

env_example = (root / "deploy/compose/env.example").read_text(encoding="utf-8")
match = re.search(r"^AI_GATEWAY_VERSION=(.+)$", env_example, re.MULTILINE)
if not match:
    raise SystemExit("deploy/compose/env.example has no AI_GATEWAY_VERSION")
checks["deploy/compose/env.example version"] = match.group(1)

errors = [
    f"{name} is {actual!r}, expected {version!r}"
    for name, actual in checks.items()
    if actual != version
]
if errors:
    raise SystemExit("\n".join(errors))

changelog = (root / "CHANGELOG.md").read_text(encoding="utf-8")
if not re.search(
    rf"^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}$",
    changelog,
    re.MULTILINE,
):
    raise SystemExit(f"CHANGELOG.md has no dated [{version}] release heading")
PY

echo "release version verified: v${version}"
