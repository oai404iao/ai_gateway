"""Offline negative oracles for the real-process adapter scenarios."""

import copy
from decimal import Decimal
import hashlib
from http.client import RemoteDisconnected
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import MagicMock, patch
from urllib.error import URLError
from urllib.request import Request

from response_adapter import (
    verify_adapter_settlement, verify_failed_wire, verify_json, verify_sse,
    verify_no_dispatch, verify_responses_json,
    verify_responses_sse,
    PLUGIN_ID, prepare_fixture,
    wire_request,
)
from run import HTTP
from mock.response_adapter import (
    AdapterUpstream, USAGE, chat_events, chat_json, responses_json, responses_events, responses_sse,
)


def adapted_sse():
    events = chat_events()
    for index in range(2):
        event = json.loads(events[index])
        event["choices"][0]["delta"]["content"] = f"adapted:{index + 1}:" + (
            "raw-first" if index == 0 else "raw-second")
        events[index] = json.dumps(event)
    return events


def wire(events):
    return "".join(f"data: {event}\n\n" for event in events).encode()


def responses_wire(events):
    return "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n"
                   for event in events).encode()


def adapted_responses_sse(terminal):
    events = responses_events(terminal)
    for index in (1, 2):
        events[index]["delta"] = f"adapted:{index}:" + ("raw-first" if index == 1 else "raw-second")
    return events


class ResponseAdapterTests(unittest.TestCase):
    def test_responses_sse_preserves_actual_terminal_without_synthesized_success(self):
        for terminal in ("completed", "incomplete", "cancelled"):
            with self.subTest(terminal=terminal):
                events = adapted_responses_sse(terminal)
                verify_responses_sse(200, responses_wire(events), "adapted:", terminal)
                failures = [events[:-1], [*events, events[-1]], responses_events(terminal)]
                for field in ("type", "status", "usage", "error"):
                    damaged = copy.deepcopy(events)
                    if field == "type":
                        damaged[-1]["type"] = "response.failed"
                    elif field == "status":
                        damaged[-1]["response"]["status"] = "in_progress"
                    elif field == "usage":
                        damaged[-1]["response"]["usage"]["input_tokens"] += 1
                    else:
                        damaged[-1]["response"]["error"] = {"code": "synthetic"}
                    failures.append(damaged)
                if terminal != "completed":
                    failures.append(adapted_responses_sse("completed"))
                for damaged in failures:
                    with self.assertRaises(RuntimeError):
                        verify_responses_sse(200, responses_wire(damaged), "adapted:", terminal)
                with self.assertRaises(RuntimeError):
                    verify_responses_sse(200, responses_wire(events) + wire(["[DONE]"]),
                                         "adapted:", terminal)

    def test_early_sse_disconnect_is_failure_evidence_not_json_or_success(self):
        data = {"public": "http://localhost"}
        with patch("response_adapter.HTTP.open", side_effect=RemoteDisconnected):
            status, payload = wire_request(data, "synthetic", stream=True)
            self.assertEqual((status, payload), (0, b""))
            verify_failed_wire(status, payload, stream=True)
            with self.assertRaises(RuntimeError):
                verify_sse(status, payload, "adapted:")
            with self.assertRaises(RemoteDisconnected):
                wire_request(data, "synthetic")

    def test_stream_timeouts_and_infrastructure_errors_are_not_adapter_success(self):
        data = {"public": "http://localhost"}
        for error in (TimeoutError(), URLError("connection refused")):
            with patch("response_adapter.HTTP.open", side_effect=error):
                with self.assertRaises(type(error)):
                    wire_request(data, "synthetic", stream=True)
        response = MagicMock()
        response.__enter__.return_value = response
        response.read.side_effect = TimeoutError()
        with patch("response_adapter.HTTP.open", return_value=response):
            with self.assertRaises(TimeoutError):
                wire_request(data, "synthetic", stream=True)

    def test_explicit_fixture_requires_matching_manifest_and_archive_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            library = Path(directory) / "libresponse_adapter.so"
            library.write_bytes(b"synthetic fixture")
            library.chmod(0o444)
            digest = hashlib.sha256(library.read_bytes()).hexdigest()
            archive = library.parent / f"{PLUGIN_ID}-test.tar.gz"
            for fault in (None, "id", "protocol", "digest", "library", "duplicate"):
                manifest = {"id": PLUGIN_ID, "protocol_version": 3,
                            "operations": ["chat_completion"]}
                build = {"library": library.name, "library_sha256": digest}
                if fault == "id":
                    manifest["id"] = "codex"
                elif fault == "protocol":
                    manifest["protocol_version"] = 1
                elif fault == "digest":
                    build["library_sha256"] = "0" * 64
                entries = [("manifest.json", json.dumps(manifest).encode()),
                           ("build-info.json", json.dumps(build).encode()),
                           (library.name, b"changed" if fault == "library" else library.read_bytes())]
                if fault == "duplicate":
                    entries.append(entries[0])
                with tarfile.open(archive, "w:gz") as package:
                    for name, payload in entries:
                        member = tarfile.TarInfo("fixture/" + name)
                        member.size = len(payload)
                        package.addfile(member, io.BytesIO(payload))
                with patch.dict("os.environ", {"AI_GATEWAY_TEST_RESPONSE_PLUGIN": str(library)}):
                    if fault:
                        with self.assertRaises(RuntimeError):
                            prepare_fixture(None)
                    else:
                        self.assertEqual(prepare_fixture(None)["sha256"], digest)

    def test_responses_json_oracle_rejects_raw_terminal_and_usage_mutation(self):
        value = responses_json()
        value["output"][0]["content"][0]["text"] = "adapted:raw-responses"
        verify_responses_json(value, "adapted:")
        for field in ("text", "usage", "terminal"):
            damaged = copy.deepcopy(value)
            if field == "text":
                damaged["output"][0]["content"][0]["text"] = "raw-responses"
            elif field == "usage":
                damaged["usage"]["input_tokens"] = 999999
            else:
                damaged["status"] = "in_progress"
            with self.assertRaises(RuntimeError):
                verify_responses_json(damaged, "adapted:")

    def test_protocol_oracle_requires_both_rejection_and_zero_dispatch(self):
        verify_no_dispatch(503, 0, 0)
        for status, before, after in ((200, 0, 0), (503, 0, 1)):
            with self.assertRaises(RuntimeError):
                verify_no_dispatch(status, before, after)

    def test_json_oracle_rejects_passthrough_usage_tampering_and_missing_terminal(self):
        value = chat_json()
        value["choices"][0]["message"]["content"] = "adapted:raw-json"
        verify_json(200, json.dumps(value).encode(), "adapted:")
        for fault in ("raw", "usage", "terminal"):
            damaged = copy.deepcopy(value)
            if fault == "raw":
                damaged["choices"][0]["message"]["content"] = "raw-json"
            elif fault == "usage":
                damaged["usage"]["prompt_tokens"] += 1
            else:
                damaged["choices"][0]["finish_reason"] = None
            with self.assertRaises(RuntimeError):
                verify_json(200, json.dumps(damaged).encode(), "adapted:")

    def test_sse_oracle_rejects_shared_state_usage_missing_and_duplicate_done(self):
        events = adapted_sse()
        verify_sse(200, wire(events), "adapted:")
        failures = [events[:-1], [*events, "[DONE]"], chat_events()]
        shared = json.loads(events[0])
        shared["choices"][0]["delta"]["content"] = "adapted:3:raw-first"
        failures.append([json.dumps(shared), *events[1:]])
        usage = json.loads(events[-2])
        usage["usage"]["completion_tokens"] += 1
        failures.append([*events[:-2], json.dumps(usage), "[DONE]"])
        for damaged in failures:
            with self.assertRaises(RuntimeError):
                verify_sse(200, wire(damaged), "adapted:")

    def test_failure_oracle_rejects_raw_fallback_or_synthesized_success(self):
        error = json.dumps({"error": {"code": "response_transform_failed"}}).encode()
        verify_failed_wire(502, error)
        verify_failed_wire(200, b"", stream=True)
        for status, payload, extra in (
            (200, json.dumps(chat_json()).encode(), {}),
            (502, error.replace(b"response_transform_failed", b"upstream_unavailable"), {}),
            (200, wire(["raw-first"]), {"stream": True}),
            (200, wire(adapted_sse()), {"stream": True}),
            (200, wire(adapted_sse()), {"stream": True, "truncated": True}),
        ):
            with self.assertRaises(RuntimeError):
                verify_failed_wire(status, payload, **extra)

    def test_loopback_fixture_is_same_format_and_bounded(self):
        with AdapterUpstream() as upstream:
            def dispatch(stream=False, path="/v1/chat/completions"):
                req = Request(upstream.url + path,
                              data=json.dumps({"model": "adapter-wire", "stream": stream}).encode(),
                              headers={"Content-Type": "application/json"})
                with HTTP.open(req, timeout=10) as response:
                    self.assertEqual(response.status, 200)
                    return response.read(65537)
            self.assertEqual(json.loads(dispatch())["usage"], USAGE)
            payload = dispatch(True)
            self.assertEqual(payload, wire(chat_events()))
            upstream.truncated = True
            self.assertNotIn(b"[DONE]", dispatch(True))
            for terminal in ("completed", "incomplete", "cancelled"):
                upstream.responses_terminal = terminal
                payload = dispatch(True, "/v1/responses")
                self.assertEqual(payload, b"".join(responses_sse(terminal)))
                self.assertNotIn(b"[DONE]", payload)
                self.assertEqual(responses_events(terminal)[-1]["response"]["status"], terminal)
            self.assertEqual(len(upstream.evidence), 6)
        self.assertFalse(upstream.thread.is_alive())

    def test_settlement_rejects_success_synthesis_changed_usage_and_balance(self):
        base = {
            "billed_at": "synthetic", "client_model": "adapter-client",
            "upstream_model": "adapter-wire", "channel_id": "channel",
            "api_operation": "chat_completion", "input_tokens": 5, "output_tokens": 2,
            "error_code": None, "outcome": "succeeded", "cost_amount": "0.000009",
            "request_protocol": "non_stream",
        }
        logs = [{**base, "request_protocol": "sse" if i < 2 else "non_stream"}
                for i in range(5)] + [
            {**base, "api_operation": "responses", "channel_id": "responses-channel"},
        ] + [
            {**base, "outcome": "failed", "error_code": "response_transform_failed",
             "cost_amount": "0", "input_tokens": 0 if i == 0 else 5,
             "output_tokens": 0 if i == 0 else 2,
             "request_protocol": "sse" if i < 2 else "non_stream"}
            for i in range(4)]
        logs += [
            {**base, "api_operation": "responses", "channel_id": "responses-channel",
             "request_protocol": "sse"},
            *[{**base, "api_operation": "responses", "channel_id": "responses-channel",
               "request_protocol": "sse", "outcome": "failed", "error_code": "upstream_sse_error",
               "cost_amount": "0"} for _ in range(2)],
        ]
        data = {"console": "http://localhost", "token": "synthetic"}
        for fault in (None, "success", "usage", "cost", "balance", "quota",
                      "upstream_kind", "upstream_usage"):
            changed = copy.deepcopy(logs)
            if fault == "success":
                changed[-1]["outcome"] = "succeeded"
            elif fault == "usage":
                changed[0]["input_tokens"] = 999999
            elif fault == "cost":
                changed[-1]["cost_amount"] = "0.000009"
            elif fault == "upstream_kind":
                changed[-1]["error_code"] = "response_transform_failed"
            elif fault == "upstream_usage":
                changed[-1]["input_tokens"] = changed[-1]["output_tokens"] = None
            def api(_base, path, **_kwargs):
                if "request-logs" in path:
                    return changed, {}
                if path.endswith("/me"):
                    return {"balance_amount": "100" if fault == "balance" else "99.999937"}, {}
                return {"quota_used_amount": "0" if fault == "quota" else "0.000063"}, {}
            with patch("response_adapter.request", side_effect=api):
                if fault:
                    with self.assertRaises(RuntimeError):
                        verify_adapter_settlement(data, "key", Decimal("100"), "channel",
                                                  "responses-channel")
                else:
                    result = verify_adapter_settlement(data, "key", Decimal("100"), "channel",
                                                       "responses-channel")
                    self.assertEqual(result["logs"], 13)


if __name__ == "__main__":
    unittest.main()
