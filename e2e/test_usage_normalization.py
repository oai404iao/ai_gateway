"""Offline wire and canonical billing oracles for connector-selected usage profiles."""

import copy
from decimal import Decimal
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.request import Request

from run import HTTP
from usage_normalization import (
    CANONICAL, PLUGIN_ID, prepare_usage_fixture, verify_usage_passthrough, verify_usage_settlement,
)
from mock.usage_normalization import (
    ANTHROPIC_USAGE, OPENAI_USAGE, MAX_REQUESTS, UsageUpstream, fixture_payload,
)


class UsageNormalizationTests(unittest.TestCase):
    def test_passthrough_rejects_reserialization_and_changed_usage_or_terminal(self):
        for profile in ("openai", "anthropic", "custom"):
            for stream in (False, True):
                expected = fixture_payload(profile, stream)
                verify_usage_passthrough(200, expected, expected)
                for status, actual in ((502, expected), (200, expected.replace(b"USAGE_OK", b"changed")),
                                       (200, expected.replace(b"completed", b"incomplete")),
                                       (200, expected + b"data: [DONE]\n\n")):
                    with self.assertRaises(RuntimeError):
                        verify_usage_passthrough(status, actual, expected)
                if not stream:
                    actual = json.dumps(json.loads(expected)).encode()
                    with self.assertRaises(RuntimeError):
                        verify_usage_passthrough(200, actual, expected)
        self.assertEqual(OPENAI_USAGE["input_tokens"], 5)
        self.assertEqual(OPENAI_USAGE["input_tokens_details"]["cached_tokens"], 3)
        self.assertEqual(ANTHROPIC_USAGE["input_tokens"], 2)
        self.assertEqual(ANTHROPIC_USAGE["cache_read_input_tokens"], 3)

    def test_loopback_profiles_are_bounded_and_do_not_adapt_response_bodies(self):
        with UsageUpstream() as upstream:
            for profile, stream in (("openai", False), ("openai", True),
                                    ("anthropic", False), ("anthropic", True), ("custom", False)):
                upstream.profile = profile
                req = Request(upstream.url + "/v1/responses",
                              data=json.dumps({"model": "usage-wire", "stream": stream}).encode(),
                              headers={"Content-Type": "application/json"})
                with HTTP.open(req, timeout=10) as response:
                    self.assertEqual(response.read(65537), fixture_payload(profile, stream))
            self.assertEqual(len(upstream.evidence), 5)
            self.assertLess(len(upstream.evidence), MAX_REQUESTS)
            self.assertFalse(upstream.errors)
        self.assertFalse(upstream.thread.is_alive())

    def test_unknown_or_interface_misclassified_usage_and_wrong_charges_fail_oracle(self):
        base = {**CANONICAL, "billed_at": "synthetic", "outcome": "succeeded",
                "response_status_code": 200, "error_code": None, "client_model": "usage-client",
                "upstream_model": "usage-wire", "channel_id": "capability",
                "api_operation": "responses", "cost_amount": "0.000006"}
        logs = [{**base, "request_protocol": "sse" if index < 2 else "non_stream"}
                for index in range(5)]
        data = {"console": "http://localhost", "token": "synthetic"}
        for fault in (None, "uncached_total", "missing_cache", "unknown", "write", "reasoning",
                      "cost", "balance", "quota"):
            changed = copy.deepcopy(logs)
            if fault == "uncached_total":
                changed[0]["input_tokens"] = 2
            elif fault == "missing_cache":
                changed[0]["cached_input_tokens"] = 0
            elif fault == "unknown":
                changed[0]["input_tokens"] = None
            elif fault == "write":
                changed[0]["cache_write_tokens"] = 1
            elif fault == "reasoning":
                changed[0]["reasoning_tokens"] = 1
            elif fault == "cost":
                changed[0]["cost_amount"] = "0.000009"
            def api(_base, path, **_kwargs):
                if "request-logs" in path:
                    return changed, {}
                if path.endswith("/me"):
                    return {"balance_amount": "100" if fault == "balance" else "99.999970"}, {}
                return {"quota_used_amount": "0" if fault == "quota" else "0.000030"}, {}
            with patch("usage_normalization.request", side_effect=api):
                if fault:
                    with self.assertRaises(RuntimeError):
                        verify_usage_settlement(data, "key", Decimal("100"), "capability")
                else:
                    result = verify_usage_settlement(data, "key", Decimal("100"), "capability")
                    self.assertEqual(result["logs"], 5)

    def test_fixture_identity_rejects_response_commands_missing_parser_and_digest_tamper(self):
        with tempfile.TemporaryDirectory() as directory:
            library = Path(directory) / "libusage_parser.so"
            library.write_bytes(b"synthetic usage fixture")
            library.chmod(0o444)
            digest = hashlib.sha256(library.read_bytes()).hexdigest()
            archive = library.parent / f"{PLUGIN_ID}-test.tar.gz"
            for fault in (None, "id", "response", "missing_parser", "digest"):
                manifest = {"id": PLUGIN_ID, "protocol_version": 3,
                            "operations": ["responses"], "commands": ["usage.parse/v1"]}
                build = {"library": library.name, "library_sha256": digest}
                if fault == "id":
                    manifest["id"] = "example-response-adapter"
                elif fault == "response":
                    manifest["commands"].append("response.json/v1")
                elif fault == "missing_parser":
                    manifest["commands"] = []
                elif fault == "digest":
                    build["library_sha256"] = "0" * 64
                entries = [("manifest.json", json.dumps(manifest).encode()),
                           ("build-info.json", json.dumps(build).encode()),
                           (library.name, library.read_bytes())]
                with tarfile.open(archive, "w:gz") as package:
                    for name, payload in entries:
                        member = tarfile.TarInfo("fixture/" + name)
                        member.size = len(payload)
                        package.addfile(member, io.BytesIO(payload))
                with patch.dict("os.environ", {"AI_GATEWAY_TEST_USAGE_PLUGIN": str(library)}):
                    if fault:
                        with self.assertRaises(RuntimeError):
                            prepare_usage_fixture(None)
                    else:
                        self.assertEqual(prepare_usage_fixture(None)["sha256"], digest)


if __name__ == "__main__":
    unittest.main()
