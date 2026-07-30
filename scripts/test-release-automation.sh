#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

python3 - <<'PY'
from pathlib import Path

expected_types = ["opened", "synchronize", "reopened"]

for relative in (
    ".github/workflows/ci.yml",
    ".github/workflows/security.yml",
):
    lines = Path(relative).read_text(encoding="utf-8").splitlines()
    try:
        start = lines.index("  pull_request:")
    except ValueError as error:
        raise AssertionError(f"{relative} has no pull_request event") from error

    block: list[str] = []
    for line in lines[start + 1 :]:
        if line and not line.startswith("    "):
            break
        block.append(line)

    try:
        types_start = block.index("    types:")
    except ValueError as error:
        raise AssertionError(
            f"{relative} must explicitly declare pull_request activity types"
        ) from error

    actual_types: list[str] = []
    for line in block[types_start + 1 :]:
        if not line.startswith("      - "):
            break
        actual_types.append(line.removeprefix("      - "))

    assert actual_types == expected_types, (
        f"{relative} pull_request types must be {expected_types}, "
        f"found {actual_types}"
    )

print("workflow pull-request event contracts passed")
PY

mkdir -p "$tmp/normalize/tools/forwarding-perf"
cp Cargo.toml Cargo.lock "$tmp/normalize/"
cp tools/forwarding-perf/Cargo.toml "$tmp/normalize/tools/forwarding-perf/"
python3 scripts/normalize-cargo-chef-version.py --root "$tmp/normalize"
python3 scripts/normalize-cargo-chef-version.py --root "$tmp/normalize"
python3 - "$tmp/normalize" "$repo_root" <<'PY'
import pathlib
import sys
import tomllib

root = pathlib.Path(sys.argv[1])
source_root = pathlib.Path(sys.argv[2])
for relative in ("Cargo.toml", "tools/forwarding-perf/Cargo.toml"):
    with (root / relative).open("rb") as handle:
        assert tomllib.load(handle)["package"]["version"] == "0.0.0"
with (root / "Cargo.lock").open("rb") as handle:
    packages = tomllib.load(handle)["package"]
with (source_root / "Cargo.lock").open("rb") as handle:
    source_packages = tomllib.load(handle)["package"]
normalized_versions = {
    package["name"]: package["version"]
    for package in packages
    if package["name"] in {"ai-gateway", "ai-gateway-perf", "adler2"}
}
source_versions = {
    package["name"]: package["version"]
    for package in source_packages
    if package["name"] == "adler2"
}
assert normalized_versions == {
    "adler2": source_versions["adler2"],
    "ai-gateway": "0.0.0",
    "ai-gateway-perf": "0.0.0",
}
PY

mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ "$1" == api && "$*" == *"/actions/workflows/ci.yml/runs?"* ]]; then
    case "${MOCK_CI_RESULT:-success}" in
        success)
            cat <<'JSON'
{"workflow_runs":[{"id":123,"path":".github/workflows/ci.yml","head_sha":"1111111111111111111111111111111111111111","head_branch":"main","event":"push","status":"completed","conclusion":"success"}]}
JSON
            ;;
        failed)
            cat <<'JSON'
{"workflow_runs":[{"id":123,"path":".github/workflows/ci.yml","head_sha":"1111111111111111111111111111111111111111","head_branch":"main","event":"push","status":"completed","conclusion":"failure"}]}
JSON
            ;;
        wrong-sha)
            cat <<'JSON'
{"workflow_runs":[{"id":123,"path":".github/workflows/ci.yml","head_sha":"2222222222222222222222222222222222222222","head_branch":"main","event":"push","status":"completed","conclusion":"success"}]}
JSON
            ;;
    esac
elif [[ "$1" == api && "$*" == *"/actions/runs/123/jobs?"* ]]; then
    if [[ "${MOCK_GATE_RESULT:-success}" == success ]]; then
        printf '%s\n' '{"jobs":[{"name":"ci-gate","status":"completed","conclusion":"success"}]}'
    else
        printf '%s\n' '{"jobs":[{"name":"ci-gate","status":"completed","conclusion":"failure"}]}'
    fi
else
    echo "unexpected gh invocation: $*" >&2
    exit 1
fi
SH
chmod +x "$tmp/bin/gh"

GH_BIN="$tmp/bin/gh" \
    GH_REPO=oai404iao/ai_gateway \
    scripts/require-successful-main-ci.sh \
    1111111111111111111111111111111111111111

if MOCK_CI_RESULT=failed \
    GH_BIN="$tmp/bin/gh" \
    GH_REPO=oai404iao/ai_gateway \
    scripts/require-successful-main-ci.sh \
    1111111111111111111111111111111111111111 \
    >/dev/null 2>&1
then
    echo "failed CI unexpectedly satisfied the release gate" >&2
    exit 1
fi

if MOCK_CI_RESULT=wrong-sha \
    GH_BIN="$tmp/bin/gh" \
    GH_REPO=oai404iao/ai_gateway \
    scripts/require-successful-main-ci.sh \
    1111111111111111111111111111111111111111 \
    >/dev/null 2>&1
then
    echo "a different commit unexpectedly satisfied the release gate" >&2
    exit 1
fi

if MOCK_GATE_RESULT=failed \
    GH_BIN="$tmp/bin/gh" \
    GH_REPO=oai404iao/ai_gateway \
    scripts/require-successful-main-ci.sh \
    1111111111111111111111111111111111111111 \
    >/dev/null 2>&1
then
    echo "failed ci-gate unexpectedly satisfied the release gate" >&2
    exit 1
fi

version="$(
    python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["package"]["version"])'
)"
mkdir -p "$tmp/package/licenses/LICENSES/example" "$tmp/package/output"
printf '#!/usr/bin/env sh\n' > "$tmp/package/ai-gateway"
chmod +x "$tmp/package/ai-gateway"
printf '%s\n' notices > "$tmp/package/licenses/THIRD_PARTY_NOTICES.md"
printf '%s\n' license > "$tmp/package/licenses/LICENSES/example/LICENSE"
RELEASE_BINARY="$tmp/package/ai-gateway" \
RELEASE_LICENSE_MATERIALS="$tmp/package/licenses" \
RELEASE_TARGET=x86_64-unknown-linux-gnu \
    scripts/package-release.sh "$version" "$tmp/package/output" >/dev/null
test -f "$tmp/package/output/ai-gateway-v${version}-x86_64-unknown-linux-gnu.tar.gz"
tar -tzf "$tmp/package/output/ai-gateway-v${version}-x86_64-unknown-linux-gnu.tar.gz" \
    | grep -Fx "ai-gateway-v${version}-x86_64-unknown-linux-gnu/ai-gateway" >/dev/null

echo "release automation tests passed"
