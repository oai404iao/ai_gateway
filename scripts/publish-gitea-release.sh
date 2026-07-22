#!/usr/bin/env bash
set -euo pipefail

tag="${1:-}"
notes_file="${2:-}"
shift 2 || true

: "${GITEA_TOKEN:?GITEA_TOKEN is required}"
: "${GITEA_API_URL:?GITEA_API_URL is required}"
: "${GITEA_REPOSITORY:?GITEA_REPOSITORY is required}"

if [[ -z "$tag" || ! -f "$notes_file" || "$#" -eq 0 ]]; then
    echo "usage: $0 <tag> <notes-file> <asset>..." >&2
    exit 2
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
response="$tmp_dir/response.json"
payload="$tmp_dir/payload.json"
release_url="${GITEA_API_URL}/repos/${GITEA_REPOSITORY}/releases"

TAG="$tag" NOTES_FILE="$notes_file" python3 - <<'PY' > "$payload"
import json
import os

tag = os.environ["TAG"]
version = tag.removeprefix("v")
with open(os.environ["NOTES_FILE"], encoding="utf-8") as handle:
    notes = handle.read().strip()

print(json.dumps({
    "tag_name": tag,
    "target_commitish": os.environ.get("GITEA_SHA", tag),
    "name": f"ai-gateway {version}",
    "body": notes,
    "draft": False,
    "prerelease": "-" in version,
}))
PY

status="$(
    curl --silent --show-error \
        --output "$response" \
        --write-out '%{http_code}' \
        --header "Authorization: token ${GITEA_TOKEN}" \
        "${release_url}/tags/${tag}"
)"
if [[ "$status" == "200" ]]; then
    release_id="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' < "$response")"
    curl --fail-with-body --silent --show-error \
        --request PATCH \
        --header "Authorization: token ${GITEA_TOKEN}" \
        --header "Content-Type: application/json" \
        --data-binary "@${payload}" \
        "${release_url}/${release_id}" \
        > "$response"
elif [[ "$status" == "404" ]]; then
    curl --fail-with-body --silent --show-error \
        --request POST \
        --header "Authorization: token ${GITEA_TOKEN}" \
        --header "Content-Type: application/json" \
        --data-binary "@${payload}" \
        "$release_url" \
        > "$response"
    release_id="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])' < "$response")"
else
    cat "$response" >&2
    echo "failed to query release $tag (HTTP $status)" >&2
    exit 1
fi

for asset in "$@"; do
    if [[ ! -f "$asset" ]]; then
        echo "missing release asset: $asset" >&2
        exit 1
    fi
    name="$(basename "$asset")"
    encoded_name="$(python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1]))' "$name")"

    existing_id="$(
        ASSET_NAME="$name" python3 -c '
import json
import os
import sys

release = json.load(sys.stdin)
for asset in release.get("assets", []):
    if asset.get("name") == os.environ["ASSET_NAME"]:
        print(asset["id"])
        break
' < "$response"
    )"
    if [[ -n "$existing_id" ]]; then
        curl --fail-with-body --silent --show-error \
            --request DELETE \
            --header "Authorization: token ${GITEA_TOKEN}" \
            "${release_url}/${release_id}/assets/${existing_id}" \
            > /dev/null
    fi

    curl --fail-with-body --silent --show-error \
        --request POST \
        --header "Authorization: token ${GITEA_TOKEN}" \
        --form "attachment=@${asset};type=application/octet-stream" \
        "${release_url}/${release_id}/assets?name=${encoded_name}" \
        > /dev/null
done

echo "published Gitea release: $tag"
