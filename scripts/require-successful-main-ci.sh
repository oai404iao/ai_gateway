#!/usr/bin/env bash
set -euo pipefail

sha="${1:-}"
shift || true

wait_seconds=0
while (($# > 0)); do
    case "$1" in
        --wait-seconds)
            wait_seconds="${2:-}"
            shift 2
            ;;
        *)
            echo "usage: $0 <commit-sha> [--wait-seconds <seconds>]" >&2
            exit 2
            ;;
    esac
done

if [[ ! "$sha" =~ ^[0-9a-f]{40}$ ]] || [[ ! "$wait_seconds" =~ ^[0-9]+$ ]]; then
    echo "usage: $0 <commit-sha> [--wait-seconds <seconds>]" >&2
    exit 2
fi

gh_bin="${GH_BIN:-gh}"
if ! command -v "$gh_bin" >/dev/null 2>&1; then
    echo "GitHub CLI is required: $gh_bin" >&2
    exit 1
fi

repo="${GH_REPO:-${GITHUB_REPOSITORY:-}}"
if [[ -z "$repo" ]]; then
    repo="$("$gh_bin" repo view --json nameWithOwner --jq .nameWithOwner)"
fi

deadline=$((SECONDS + wait_seconds))
while true; do
    if ! runs_json="$(
        "$gh_bin" api \
            -H "Accept: application/vnd.github+json" \
            "repos/${repo}/actions/workflows/ci.yml/runs?branch=main&event=push&per_page=100"
    )"; then
        if ((SECONDS >= deadline)); then
            echo "unable to query main CI runs for ${sha}" >&2
            exit 1
        fi
        sleep 10
        continue
    fi
    mapfile -t run_ids < <(
        python3 -c '
import json
import sys

sha = sys.argv[1]
data = json.load(sys.stdin)
for run in data.get("workflow_runs", []):
    if (
        run.get("head_sha") == sha
        and run.get("head_branch") == "main"
        and run.get("event") == "push"
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("path") == ".github/workflows/ci.yml"
    ):
        print(run["id"])
' "$sha" <<<"$runs_json"
    )

    for run_id in "${run_ids[@]}"; do
        if ! jobs_json="$(
            "$gh_bin" api \
                -H "Accept: application/vnd.github+json" \
                "repos/${repo}/actions/runs/${run_id}/jobs?per_page=100"
        )"; then
            continue
        fi
        if python3 -c '
import json
import sys

data = json.load(sys.stdin)
raise SystemExit(
    0
    if any(
        job.get("name") == "ci-gate"
        and job.get("status") == "completed"
        and job.get("conclusion") == "success"
        for job in data.get("jobs", [])
    )
    else 1
)
' <<<"$jobs_json"; then
            echo "verified successful main ci-gate for ${sha}: run ${run_id}"
            exit 0
        fi
    done

    if ((SECONDS >= deadline)); then
        echo "no successful main ci-gate found for ${sha}" >&2
        exit 1
    fi
    sleep 10
done
