#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

python3 - <<'PY'
from pathlib import Path

import yaml

expected_types = {"opened", "synchronize", "reopened"}


def check_pull_request_types(source, name):
    # BaseLoader preserves `on` as a string rather than a YAML 1.1 boolean.
    workflow = yaml.load(source, Loader=yaml.BaseLoader)
    actual = workflow.get("on", {}).get("pull_request", {}).get("types")
    assert isinstance(actual, list) and set(actual) == expected_types, (
        f"{name} pull_request types must be {sorted(expected_types)}, found {actual}"
    )


for relative in (
    ".github/workflows/ci.yml",
    ".github/workflows/security.yml",
):
    check_pull_request_types(Path(relative).read_text(encoding="utf-8"), relative)

flow = "on: {pull_request: {types: [reopened, opened, synchronize]}, push: {}}"
check_pull_request_types(flow, "flow style")
for label, source in (
    ("missing activity", flow.replace(", synchronize", "")),
    ("extra activity", flow.replace("synchronize", "synchronize, closed")),
):
    try:
        check_pull_request_types(source, label)
    except AssertionError:
        pass
    else:
        raise AssertionError(f"{label} unexpectedly passed")

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
python3 - "$tmp/version" "$version" <<'PY'
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(sys.argv[1])
version = sys.argv[2]
checker = "scripts/check-release-version.sh"
compose = "docker-compose.prd.yaml"
example = "deploy/compose/env.example"
for relative in (checker, compose, example, "Cargo.toml", "Cargo.lock", "CHANGELOG.md",
                 "web/console/package.json", "tools/forwarding-perf/Cargo.toml"):
    target = root / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(relative, target)


def verify():
    return subprocess.run(
        ["bash", str(root / checker), version], capture_output=True, text=True,
        env={**os.environ, "AI_GATEWAY_VERSION": mismatch},
    )


mismatch = "0.0.1" if version != "0.0.1" else "0.0.2"
(root / ".env").write_text(f"AI_GATEWAY_VERSION={mismatch}\n", encoding="utf-8")
positive = verify()
assert positive.returncode == 0, positive.stderr
def compose_fixture(image_version, build_version):
    return json.dumps({"services": {"gateway": {
        "image": f"fixture:${{AI_GATEWAY_VERSION:-{image_version}}}",
        "build": {"context": ".", "args": {"VERSION": f"${{AI_GATEWAY_VERSION:-{build_version}}}"}},
    }}})


example_text = Path(example).read_text(encoding="utf-8")
for relative, modified, diagnostic in (
    (compose, compose_fixture(mismatch, version), "gateway image tag"),
    (compose, compose_fixture(version, mismatch), "gateway build VERSION"),
    (example, example_text + f"\nexport AI_GATEWAY_VERSION = {version}\n", "exactly once"),
    (example, example_text + f"\nAI_GATEWAY_VERSION={mismatch}\n", "exactly once"),
    (example, f"\ufeffAI_GATEWAY_VERSION={mismatch}\n{example_text}", "exactly once"),
    (example, example_text + f"\n\u00a0AI_GATEWAY_VERSION={mismatch}\n", "exactly once"),
    (example, f"AI_GATEWAY_VERSION='{mismatch}'\n", f"version is '{mismatch}'"),
    (example, f'AI_GATEWAY_VERSION={mismatch}\nZZZ_NOTE="line\\nAI_GATEWAY_VERSION={version}\\nend"\n',
     f"version is '{mismatch}'"),
):
    target = root / relative
    target.write_text(modified, encoding="utf-8")
    result = verify()
    assert result.returncode != 0 and diagnostic in result.stderr, (diagnostic, result)
    shutil.copyfile(relative, target)
PY

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
python3 scripts/test-third-party-notices.py
