"""Connector-selected pure usage normalization with unchanged wire response bytes."""

from collections import Counter
from decimal import Decimal
import hashlib
import json
import os
from pathlib import Path
import tarfile
from urllib.request import Request

from run import HTTP, ROOT, check, load_plugin_fixture, request, wait_until
from mock.usage_normalization import UsageUpstream, fixture_payload

PLUGIN_ID = "example-usage-parser"
MAX_WIRE = 65536
CANONICAL = {
    "input_tokens": 5, "cached_input_tokens": 3, "cache_write_tokens": 0,
    "output_tokens": 2, "reasoning_tokens": 0,
}


def prepare_usage_fixture(resources):
    path = os.environ.get("AI_GATEWAY_TEST_USAGE_PLUGIN")
    if not path:
        path = resources.run(
            ["bash", str(ROOT / "scripts/prepare-usage-parser-tests.sh")],
            "prepare-usage-parser", timeout=300,
            env={**os.environ, "TMPDIR": resources.env["TMPDIR"]},
        ).strip().splitlines()[-1]
    fixture = load_plugin_fixture(path, PLUGIN_ID)
    archive = Path(path).parent / f"{PLUGIN_ID}-test.tar.gz"
    check(archive.is_file() and archive.stat().st_size <= 256 * 1024 * 1024,
          "usage parser package missing; run scripts/prepare-usage-parser-tests.sh")
    with tarfile.open(archive) as package:
        members = package.getmembers()
        check(len(members) <= 128, "usage parser package member limit exceeded")
        def read_member(name, limit):
            found = [member for member in members if member.name.endswith("/" + name)]
            check(len(found) == 1 and found[0].isfile() and found[0].size <= limit,
                  "invalid usage parser package member")
            return package.extractfile(found[0]).read(limit + 1)
        manifest = json.loads(read_member("manifest.json", 65536))
        build = json.loads(read_member("build-info.json", 65536))
        check(manifest["id"] == PLUGIN_ID and manifest["protocol_version"] == 3
              and "responses" in manifest["operations"] and "usage.parse/v1" in manifest["commands"]
              and not any(command.startswith("response.") for command in manifest["commands"]),
              "usage parser manifest mismatch")
        check(build["library_sha256"] == fixture["sha256"], "usage parser package digest mismatch")
        check(hashlib.sha256(read_member(build["library"], 256 * 1024 * 1024)).hexdigest()
              == fixture["sha256"], "usage parser archive library mismatch")
    return {**fixture, "archive": str(archive)}


def usage_request(data, key, stream):
    req = Request(data["public"] + "/v1/responses", method="POST",
                  headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
                  data=json.dumps({"model": "usage-client", "input": "synthetic", "stream": stream}).encode())
    with HTTP.open(req, timeout=15) as response:
        payload = response.read(MAX_WIRE + 1)
        check(len(payload) <= MAX_WIRE, "usage wire evidence limit exceeded")
        return response.status, payload


def verify_usage_passthrough(status, payload, expected):
    check(status == 200 and payload == expected, "usage normalization changed upstream wire response")


def verify_usage_settlement(data, key_id, baseline, capability_id):
    logs = []
    def ready():
        nonlocal logs
        logs = request(data["console"], f"/console/v1/request-logs?api_key_id={key_id}",
                       token=data["token"])[0]
        return len(logs) == 5 and all(log["billed_at"] for log in logs)
    wait_until(ready)
    check(Counter(log["request_protocol"] for log in logs) == {"non_stream": 3, "sse": 2},
          "usage normalization protocol count mismatch")
    for log in logs:
        check(log["outcome"] == "succeeded" and log["response_status_code"] == 200
              and log["error_code"] is None, "usage normalization request failed")
        check(log["client_model"] == "usage-client" and log["upstream_model"] == "usage-wire"
              and log["channel_id"] == capability_id and log["api_operation"] == "responses",
              "usage normalization route log mismatch")
        check(all(log[field] == value for field, value in CANONICAL.items()),
              "connector/interface canonical usage mismatch")
        check(Decimal(log["cost_amount"]) == Decimal("0.000006"), "usage normalization cost mismatch")
    cost = Decimal("0.000030")
    me = request(data["console"], "/console/v1/me", token=data["token"])[0]
    key = request(data["console"], f"/console/v1/api-keys/{key_id}", token=data["token"])[0]
    check(Decimal(me["balance_amount"]) == baseline - cost, "usage normalization balance mismatch")
    check(Decimal(key["quota_used_amount"]) == cost, "usage normalization quota mismatch")
    return {"logs": len(logs), "cost": str(cost), "canonical_usage": CANONICAL,
            "upstream_bytes_unchanged": True}


def exercise_usage_normalization(resources, data, fixture):
    def api(path, method="GET", body=None, etag=None):
        return request(data["console"], "/console/v1" + path, method, body, data["token"], etag)

    def write(path, method, body, etag=None):
        auth, _ = api("/plugins/reauth", "POST", {"password": data["password"]})
        return request(data["console"], "/console/v1" + path, method, body, data["token"],
                       etag, auth["token"])

    job, _ = write("/plugins/install", "POST", Path(fixture["archive"]).read_bytes())
    def installed():
        result, _ = api(f"/plugins/jobs/{job['id']}")
        check(result["status"] != "failed", "usage parser installation failed")
        return result["status"] == "succeeded"
    wait_until(installed)
    path = "/plugins/" + PLUGIN_ID
    detail, headers = api(path)
    check(not detail["enabled"] and any(item["digest"] == fixture["sha256"]
                                       for item in detail["artifacts"]),
          "usage parser installed identity mismatch")
    write(path + "/state", "PUT", {"enabled": True, "artifact_digest": fixture["sha256"]},
          headers["ETag"])
    baseline = Decimal(api("/me")[0]["balance_amount"])
    with UsageUpstream() as upstream:
        group, _ = api("/routing/groups", "POST", {"name": "usage-parser", "enabled": True})
        access, _ = api("/routing/accesses", "POST", {
            "name": "usage-parser", "connector_kind": PLUGIN_ID,
            "base_url": upstream.url, "enabled": True})
        channel, _ = api("/routing/logical-channels", "POST", {
            "group_id": group["id"], "access_id": access["id"], "name": "usage-parser",
            "credential_id": None, "enabled": True})
        capability, _ = api("/routing/capabilities", "POST", {
            "channel_id": channel["id"], "settings": {
                "operation": "responses", "enabled": True, "available_models": ["usage-wire"],
                "request_compression": "default", "auto_disable_allowed": False,
                "test_model": None, "test_pricing_model_id": None}})
        model, _ = api("/models", "POST", {
            "source_model_id": "usage-client", "display_name": "Usage normalization", "enabled": True,
            "price_unit_tokens": 1000000, "input_unit_price": "1", "cached_input_unit_price": "0",
            "cache_write_unit_price": "0", "output_unit_price": "2",
            "price_effective_at": "2026-01-01T00:00:00Z"})
        profile, _ = api("/routing/profiles", "POST", {"model_id": model["id"]})
        api("/routing/operation-rules", "POST", {
            "model_routing_profile_id": profile["id"], "operation": "responses", "enabled": True,
            "routing_tiers": [{"priority": 0, "selection_strategy": "weighted_round_robin",
                              "candidates": [{"capability_id": capability["id"],
                                              "upstream_model": "usage-wire", "weight": 1}]}]})
        key, _ = api("/api-keys", "POST", {
            "user_id": data["user_id"], "name": "usage-normalization", "permissions": ["proxy"],
            "allowed_group_ids": [group["id"]], "allowed_channel_ids": [],
            "allowed_api_formats": ["open_ai_responses"]})
        resources.secret_values.append(key["secret"])
        scenarios = []
        for parser_profile, streams in (("openai", (False, True)), ("anthropic", (False, True)),
                                        ("custom", (False,))):
            settings, headers = api(path + "/settings")
            write(path + "/settings", "PUT", {
                "schema_version": settings["schema_version"], "values": {"profile": parser_profile}},
                headers["ETag"])
            upstream.profile = parser_profile
            for stream in streams:
                expected = fixture_payload(parser_profile, stream)
                verify_usage_passthrough(*usage_request(data, key["secret"], stream), expected)
                scenarios.append({"id": f"plugin-usage-{parser_profile}-{'sse' if stream else 'json'}",
                                  "status": "passed"})
        check(len(upstream.evidence) == 5 and not upstream.errors, "usage fixture dispatch mismatch")
        settlement = verify_usage_settlement(data, key["id"], baseline, capability["id"])
        return scenarios, settlement
