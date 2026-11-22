"""Real-process native adapter acceptance, with independent wire and billing oracles."""

from concurrent.futures import ThreadPoolExecutor
from collections import Counter
from decimal import Decimal
import hashlib
from http.client import IncompleteRead, RemoteDisconnected
import json
import os
from pathlib import Path
import tarfile
import threading
from urllib.error import HTTPError
from urllib.request import Request

from run import HTTP, ROOT, check, load_plugin_fixture, request, settled_request_logs, wait_until
from mock.response_adapter import AdapterUpstream, USAGE, responses_events

PLUGIN_ID = "example-response-adapter"
MAX_WIRE = 65536


def prepare_fixture(resources):
    path = os.environ.get("AI_GATEWAY_TEST_RESPONSE_PLUGIN")
    if not path:
        path = resources.run(
            ["bash", str(ROOT / "scripts/prepare-response-adapter-tests.sh")],
            "prepare-response-adapter", timeout=300,
            env={**os.environ, "TMPDIR": resources.env["TMPDIR"]},
        ).strip().splitlines()[-1]
    fixture = load_plugin_fixture(path, PLUGIN_ID)
    archive = Path(path).parent / f"{PLUGIN_ID}-test.tar.gz"
    check(archive.is_file() and archive.stat().st_size <= 256 * 1024 * 1024,
          "response adapter package missing; run scripts/prepare-response-adapter-tests.sh")
    # Never extract an explicitly supplied archive in the harness.
    with tarfile.open(archive) as package:
        members = package.getmembers()
        check(len(members) <= 128, "response adapter package member limit exceeded")
        def read_member(name, limit):
            found = [member for member in members if member.name.endswith("/" + name)]
            check(len(found) == 1 and found[0].isfile() and found[0].size <= limit,
                  "invalid response adapter package member")
            return package.extractfile(found[0]).read(limit + 1)
        manifest = json.loads(read_member("manifest.json", 65536))
        build = json.loads(read_member("build-info.json", 65536))
        check(manifest["id"] == PLUGIN_ID and manifest["protocol_version"] == 3
              and "chat_completion" in manifest["operations"], "response adapter manifest mismatch")
        check(build["library_sha256"] == fixture["sha256"], "response adapter package digest mismatch")
        check(hashlib.sha256(read_member(build["library"], 256 * 1024 * 1024)).hexdigest()
              == fixture["sha256"], "response adapter archive library mismatch")
    return {**fixture, "archive": str(archive)}


def wire_request(data, key, stream=False, operation="chat_completion"):
    check(operation in ("chat_completion", "responses"), "unsupported fixture operation")
    body = {"model": "adapter-client", "stream": stream}
    if operation == "responses":
        body["input"] = "synthetic"
        path = "/v1/responses"
    else:
        body["messages"] = [{"role": "user", "content": "synthetic"}]
        path = "/v1/chat/completions"
    if stream and operation == "chat_completion":
        body["stream_options"] = {"include_usage": True}
    req = Request(data["public"] + path, method="POST",
                  headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
                  data=json.dumps(body).encode())
    try:
        response = HTTP.open(req, timeout=15)
    except HTTPError as error:
        response = error
    except (RemoteDisconnected, ConnectionResetError):
        if not stream:
            raise
        return 0, b""
    with response:
        if stream:
            wire = bytearray()
            try:
                while len(wire) <= MAX_WIRE:
                    chunk = response.read1(MAX_WIRE + 1 - len(wire))
                    if not chunk:
                        break
                    wire.extend(chunk)
            except IncompleteRead as error:
                wire.extend(error.partial)
            except ConnectionResetError:
                pass
            wire = bytes(wire)
        else:
            wire = response.read(MAX_WIRE + 1)
        check(len(wire) <= MAX_WIRE, "adapter wire evidence limit exceeded")
        return response.status, wire


def sse_events(wire):
    name, data = "", []
    for line in wire.decode().replace("\r\n", "\n").replace("\r", "\n").splitlines(keepends=True):
        if line == "\n":
            if data:
                yield name, "\n".join(data)
            name, data = "", []
        else:
            field, _, value = line.removesuffix("\n").partition(":")
            if field == "event":
                name = value.removeprefix(" ")
            elif field == "data":
                data.append(value.removeprefix(" "))


def chat_deltas(events):
    return [choice["delta"]["content"] for event in events for choice in event.get("choices", [])
            if "content" in choice.get("delta", {})]


def verify_json(status, wire, label):
    check(status == 200, "adapted JSON status mismatch")
    value = json.loads(wire)
    check(value["choices"][0]["message"]["content"] == label + "raw-json",
          "JSON text was not adapted")
    check(value["choices"][0]["finish_reason"] == "stop" and value["usage"] == USAGE,
          "JSON usage or terminal changed")


def verify_responses_json(value, label):
    check(value["output"][0]["content"][0]["text"] == label + "raw-responses"
          and value["status"] == "completed" and value["usage"] == {
              "input_tokens": 5, "output_tokens": 2, "total_tokens": 7},
          "Responses JSON text, terminal or usage was not preserved")


def verify_no_dispatch(status, wire, arrived):
    check(status == 503, "expected route rejection for unsupported protocol")
    check(json.loads(wire)["error"]["code"] == "no_healthy_channel",
          "unexpected unsupported protocol error")
    check(not arrived, "unsupported protocol dispatched upstream")


def verify_sse(status, wire, label):
    check(status == 200, "adapted SSE status mismatch")
    values = [value for _, value in sse_events(wire)]
    check(values and values[-1] == "[DONE]" and values.count("[DONE]") == 1,
          "SSE terminal missing or synthesized twice")
    events = [json.loads(value) for value in values[:-1]]
    check(chat_deltas(events) == [label + "1:raw-first", label + "2:raw-second"],
          "SSE text or request-local state mismatch")
    check([event["usage"] for event in events if event.get("usage")] == [USAGE],
          "SSE usage changed")
    check(sum(choice.get("finish_reason") == "stop" for event in events
              for choice in event.get("choices", [])) == 1, "SSE finish reason changed")


def verify_responses_sse(status, wire, label, terminal="completed"):
    check(status == 200, "Responses SSE status mismatch")
    frames = list(sse_events(wire))
    values = [value for _, value in frames]
    check("[DONE]" not in values, "Responses SSE synthesized DONE")
    events = [json.loads(value) for value in values]
    expected = responses_events(terminal)
    check(len(events) == len(expected), "Responses SSE missing or synthesized event")
    for index in (1, 2):
        expected[index]["delta"] = f"{label}{index}:" + ("raw-first" if index == 1 else "raw-second")
    check(events == expected, "Responses SSE text, usage or terminal mutated")
    names = [name for name, _ in frames]
    check(names == [event["type"] for event in expected], "Responses SSE event name mismatch")


def verify_failed_wire(status, wire, stream=False, *, truncated_label=None):
    if truncated_label is not None:
        check(status == 200, "truncation must follow successful SSE response headers")
        values = [value for _, value in sse_events(wire)]
        check("[DONE]" not in values, "truncated stream synthesized success")
        text = chat_deltas(json.loads(value) for value in values)
        expected = [truncated_label + "1:raw-first", truncated_label + "2:raw-second"]
        check(bool(text) and text == expected[:len(text)], "truncated SSE leaked raw or incorrect deltas")
    elif stream and status in (0, 200):
        # A body failure on the first frame can close the connection before
        # headers escape, or after headers but before any SSE payload.
        check(not wire, "first-event adapter failure emitted a body")
    else:
        check(status == 502, "unexpected HTTP status for adapter failure")
        check(json.loads(wire)["error"]["code"] == "response_transform_failed", "wrong adapter failure")
        check(all(raw not in wire for raw in (b"raw-json", b"raw-first", b"raw-second")),
              "invalid adapter fell back to raw content")


def verify_adapter_settlement(data, key_id, baseline, channel_id, responses_channel_id, transform_failures):
    logs = settled_request_logs(data, key_id, 13)
    successes = [log for log in logs if log["outcome"] == "succeeded"]
    failures = [log for log in logs if log["outcome"] == "failed"
                and log["error_code"] == "response_transform_failed"]
    upstream_failures = [log for log in logs if log["outcome"] == "failed"
                         and log["error_code"] == "upstream_sse_error"]
    check(len(successes) == 7 and len(failures) == 4 and len(upstream_failures) == 2,
          "adapter durable outcomes mismatch")
    check(Counter(log["request_protocol"] for log in logs) == {"non_stream": 6, "sse": 7},
          "adapter protocol count mismatch")
    response_logs = [log for log in logs if log["api_operation"] == "responses"]
    check(len(response_logs) == 4 and sum(log in successes for log in response_logs) == 2
          and all(log in response_logs and log["request_protocol"] == "sse"
                  for log in upstream_failures),
          "Responses adapter request was not independently settled")
    check({log["id"] for log in failures} == set(transform_failures), "adapter failure request IDs mismatch")
    for log in logs:
        check(log["client_model"] == "adapter-client" and log["upstream_model"] == "adapter-wire"
              and ((log["channel_id"] == channel_id and log["api_operation"] == "chat_completion")
                   or (log["channel_id"] == responses_channel_id and log["api_operation"] == "responses")),
              "adapter routing log mismatch")
        tokens = (log["input_tokens"], log["output_tokens"])
        if log["id"] in transform_failures:
            protocol, known = transform_failures[log["id"]]
            check(log["request_protocol"] == protocol, "adapter failure protocol mismatch")
        else:
            known = True
        check(tokens in (((5, 2),) if known else ((0, 0), (None, None))),
              "adapter scenario lost known usage or synthesized unknown usage")
        check(Decimal(log["cost_amount"]) == (Decimal("0.000009") if log in successes else 0),
              "adapter settlement cost mismatch")
    cost = Decimal("0.000009") * len(successes)
    me = request(data["console"], "/console/v1/me", token=data["token"])[0]
    key = request(data["console"], f"/console/v1/api-keys/{key_id}", token=data["token"])[0]
    check(Decimal(me["balance_amount"]) == baseline - cost, "adapter balance mismatch")
    check(Decimal(key["quota_used_amount"]) == cost, "adapter key quota mismatch")
    return {"logs": len(logs), "succeeded": 7, "failed": 6, "cost": str(cost),
            "raw_usage_authoritative": True}


def exercise_response_adapter(resources, data, fixture):
    def api(path, method="GET", body=None, etag=None):
        return request(data["console"], "/console/v1" + path, method, body, data["token"], etag)

    def write(path, method, body, etag=None):
        auth, _ = api("/plugins/reauth", "POST", {"password": data["password"]})
        return request(data["console"], "/console/v1" + path, method, body, data["token"],
                       etag, auth["token"])

    job, _ = write("/plugins/install", "POST", Path(fixture["archive"]).read_bytes())
    def installed():
        result, _ = api(f"/plugins/jobs/{job['id']}")
        check(result["status"] != "failed", "response adapter installation failed")
        return result["status"] == "succeeded"
    wait_until(installed)
    path = "/plugins/" + PLUGIN_ID
    detail, headers = api(path)
    check(not detail["enabled"] and any(item["digest"] == fixture["sha256"]
                                       for item in detail["artifacts"]),
          "response adapter installed identity mismatch")
    write(path + "/state", "PUT", {"enabled": True, "artifact_digest": fixture["sha256"]},
          headers["ETag"])

    def settings(**changes):
        current, headers = api(path + "/settings")
        values = {"label": "adapted:", "mode": "normal", "supported_protocols": "both", **changes}
        write(path + "/settings", "PUT", {"schema_version": current["schema_version"],
                                        "values": values}, headers["ETag"])

    baseline = Decimal(api("/me")[0]["balance_amount"])
    with AdapterUpstream() as upstream:
        group, _ = api("/routing/groups", "POST", {"name": "response-adapter", "enabled": True})
        access, _ = api("/routing/accesses", "POST", {
            "name": "response-adapter", "connector_kind": PLUGIN_ID,
            "base_url": upstream.url, "enabled": True})
        channel, _ = api("/routing/logical-channels", "POST", {
            "group_id": group["id"], "access_id": access["id"], "name": "response-adapter",
            "credential_id": None, "enabled": True})
        capability, _ = api("/routing/capabilities", "POST", {
            "channel_id": channel["id"], "settings": {
                "operation": "chat_completion", "enabled": True,
                "available_models": ["adapter-wire"], "request_compression": "default",
                "auto_disable_allowed": False, "test_model": None, "test_pricing_model_id": None}})
        model, _ = api("/models", "POST", {
            "source_model_id": "adapter-client", "display_name": "Response adapter", "enabled": True,
            "price_unit_tokens": 1000000, "input_unit_price": "1", "cached_input_unit_price": "0",
            "cache_write_unit_price": "0", "output_unit_price": "2",
            "price_effective_at": "2026-01-01T00:00:00Z"})
        profile, _ = api("/routing/profiles", "POST", {"model_id": model["id"]})
        api("/routing/operation-rules", "POST", {
            "model_routing_profile_id": profile["id"], "operation": "chat_completion", "enabled": True,
            "routing_tiers": [{"priority": 0, "selection_strategy": "weighted_round_robin",
                              "candidates": [{"capability_id": capability["id"],
                                              "upstream_model": "adapter-wire", "weight": 1}]}]})
        response_capability, _ = api("/routing/capabilities", "POST", {
            "channel_id": channel["id"], "settings": {
                "operation": "responses", "enabled": True, "available_models": ["adapter-wire"],
                "request_compression": "default", "auto_disable_allowed": False,
                "test_model": None, "test_pricing_model_id": None}})
        api("/routing/operation-rules", "POST", {
            "model_routing_profile_id": profile["id"], "operation": "responses", "enabled": True,
            "routing_tiers": [{"priority": 0, "selection_strategy": "weighted_round_robin",
                              "candidates": [{"capability_id": response_capability["id"],
                                              "upstream_model": "adapter-wire", "weight": 1}]}]})
        def key(name):
            result, _ = api("/api-keys", "POST", {
                "user_id": data["user_id"], "name": name, "permissions": ["proxy"],
                "allowed_group_ids": [group["id"]], "allowed_channel_ids": []})
            resources.secret_values.append(result["secret"])
            return result
        main_key, denied_key = key("response-adapter"), key("response-adapter-denied")
        settings(supported_protocols="non_stream")
        status, wire = wire_request(data, denied_key["secret"], stream=True)
        verify_no_dispatch(status, wire, upstream.arrived.is_set())
        check(Decimal(api(f"/api-keys/{denied_key['id']}")[0]["quota_used_amount"]) == 0,
              "unsupported protocol charged quota")
        scenarios = [{"id": "plugin-response-capability-predispatch", "status": "passed", "dispatches": 0}]
        settings()
        verify_json(*wire_request(data, main_key["secret"]), "adapted:")
        scenarios.append({"id": "plugin-response-json-raw-metering", "status": "passed"})
        response, _ = request(data["public"], "/v1/responses", "POST", {
            "model": "adapter-client", "input": "synthetic", "stream": False},
            token=main_key["secret"])
        verify_responses_json(response, "adapted:")
        scenarios.append({"id": "plugin-response-responses-json-raw-metering", "status": "passed"})
        for terminal, scenario in (
            ("completed", "plugin-response-responses-sse-raw-metering"),
            ("incomplete", "plugin-response-responses-incomplete-preserved"),
            ("cancelled", "plugin-response-responses-cancelled-preserved"),
        ):
            upstream.responses_terminal = terminal
            verify_responses_sse(*wire_request(data, main_key["secret"], True, "responses"),
                                 "adapted:", terminal)
            scenarios.append({"id": scenario, "status": "passed"})
        upstream.interleave = threading.Barrier(2)
        with ThreadPoolExecutor(max_workers=2) as clients:
            futures = [clients.submit(wire_request, data, main_key["secret"], True) for _ in range(2)]
            for future in futures:
                verify_sse(*future.result(timeout=20), "adapted:")
        upstream.interleave = None
        scenarios.append({"id": "plugin-response-sse-interleaved-state", "status": "passed", "requests": 2})
        upstream.arrived.clear()
        upstream.release.clear()
        with ThreadPoolExecutor(max_workers=1) as clients:
            future = clients.submit(wire_request, data, main_key["secret"])
            try:
                check(upstream.arrived.wait(timeout=10), "pinned request did not dispatch")
                settings(label="next:")
            finally:
                upstream.release.set()
            verify_json(*future.result(timeout=20), "adapted:")
        verify_json(*wire_request(data, main_key["secret"]), "next:")
        scenarios.append({"id": "plugin-response-settings-inflight-pinned", "status": "passed"})
        seen = {log["id"] for log in settled_request_logs(data, main_key["id"], 9)}
        transform_failures = {}

        def record_failure(protocol, known_usage):
            ids = {log["id"] for log in settled_request_logs(data, main_key["id"], len(seen) + 1)}
            check(seen < ids, "adapter failure did not append exactly one log")
            [request_id] = ids - seen
            seen.add(request_id)
            transform_failures[request_id] = (protocol, known_usage)

        for mode, stream, scenario in (
            ("invalid", False, "plugin-response-invalid-json-failclosed"),
            ("metering_tamper", False, "plugin-response-metering-tamper-failclosed"),
            ("invalid", True, "plugin-response-invalid-sse-failclosed"),
        ):
            settings(mode=mode)
            verify_failed_wire(*wire_request(data, main_key["secret"], stream), stream=stream)
            record_failure("sse" if stream else "non_stream", known_usage=not stream)
            scenarios.append({"id": scenario, "status": "passed"})
        settings()
        upstream.truncated = True
        verify_failed_wire(*wire_request(data, main_key["secret"], True), truncated_label="adapted:")
        record_failure("sse", known_usage=True)
        scenarios.append({"id": "plugin-response-truncation-no-success", "status": "passed"})
        check(len(upstream.evidence) == 13 and not upstream.errors, "adapter upstream evidence mismatch")
        settlement = verify_adapter_settlement(data, main_key["id"], baseline, capability["id"],
                                               response_capability["id"], transform_failures)
        return scenarios, settlement
