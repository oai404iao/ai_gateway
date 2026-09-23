"""Offline contract and lifecycle tests; never start Docker or paid upstreams."""

import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch

from run import Resources, redact, verify_settlement
from mock.upstream import CONTRACT, MAX_REQUESTS, Scenario, Upstream
from run import request


def tool_request(**extra):
    return {
        "model": "e2e-wire", "stream": True,
        "tools": [{"type": "function", "name": "exec_command"}], **extra,
    }


class ScenarioTests(unittest.TestCase):
    def test_actual_tool_output_required(self):
        fixture = Scenario("random-marker")
        events, response = fixture.respond(tool_request())
        call = response["output"][0]
        self.assertEqual(call["type"], "function_call")
        self.assertEqual(json.loads(call["arguments"])["cmd"], "cat marker.txt")
        self.assertNotIn("random-marker", json.dumps(events))
        self.assertFalse(fixture.tool_completed)
        fixture.respond(tool_request(input=[{
            "type": "function_call_output", "call_id": call["call_id"],
            "output": "Process exited with code 0\nrandom-marker",
        }]))
        self.assertTrue(fixture.tool_completed)
        self.assertEqual(len(fixture.evidence), 2)
        with self.assertRaisesRegex(ValueError, "third"):
            fixture.respond(tool_request())

    def test_fake_final_text_wrong_call_id_and_wrong_output_fail(self):
        for entry in (
            {"type": "message", "role": "assistant", "content": "E2E_TOOL_OK"},
            {"type": "function_call_output", "call_id": "wrong", "output": "random-marker"},
            {"type": "function_call_output", "call_id": "call_system_e2e", "output": "error"},
        ):
            fixture = Scenario("random-marker")
            fixture.respond(tool_request())
            with self.assertRaisesRegex(ValueError, "tool output missing"):
                fixture.respond(tool_request(input=[entry]))
            self.assertFalse(fixture.tool_completed)

    def test_wrong_model_unknown_tool_and_request_limit_fail(self):
        fixture = Scenario("marker")
        with self.assertRaisesRegex(ValueError, "model rewrite"):
            fixture.respond({"model": "e2e-client"})
        with self.assertRaisesRegex(ValueError, "supported shell"):
            fixture.respond(tool_request(tools=[{"type": "function", "name": "network"}]))
        for _ in range(MAX_REQUESTS * 2):
            try:
                fixture.respond({"model": "e2e-wire"})
            except ValueError:
                pass
        self.assertLessEqual(len(fixture.errors), MAX_REQUESTS)
        self.assertLessEqual(len(fixture.evidence), MAX_REQUESTS)

    def test_stream_lifecycle_and_usage(self):
        fixture = Scenario("marker")
        events, response = fixture.respond({"model": "e2e-wire", "stream": True})
        self.assertEqual(events[0]["type"], "response.created")
        self.assertEqual(events[-1]["type"], "response.completed")
        self.assertEqual(events[-1]["response"]["status"], "completed")
        self.assertEqual(response["usage"], {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7})
        self.assertEqual([event["sequence_number"] for event in events], list(range(len(events))))
        self.assertEqual(response["output"][0]["content"][0]["text"], "E2E_TEXT_OK")
        self.assertEqual(CONTRACT["version"], 1)

    def test_http_fixture_and_shutdown(self):
        with Upstream("marker") as fixture:
            body, _ = request(fixture.url, "/v1/responses", "POST", {"model": "e2e-wire"})
            self.assertEqual(body["status"], "completed")
            with self.assertRaisesRegex(RuntimeError, "HTTP 400"):
                request(fixture.url, "/unexpected", "POST", {"model": "e2e-wire"})
        self.assertFalse(fixture.thread.is_alive())


class HarnessTests(unittest.TestCase):
    def test_child_temp_directory_does_not_enclose_client_homes(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Resources(Path(directory))
            temporary = Path(resources.env["TMPDIR"])
            self.assertTrue(temporary.is_dir())
            self.assertTrue(temporary.is_relative_to(resources.directory))
            for name in ("codex-http-home", "codex-ws-home"):
                self.assertFalse((resources.directory / name).is_relative_to(temporary))
            self.assertEqual(resources.close(), [])

    def test_redaction(self):
        text = redact("postgres://u:secret@host/db Bearer jwt-token\npassword secret", ["secret"])
        self.assertNotIn("secret", text)
        self.assertNotIn("jwt-token", text)

    def test_failed_and_timed_out_children_are_reaped(self):
        for code, timeout, expected in [
            ("raise SystemExit(3)", 5, RuntimeError),
            ("import time; time.sleep(60)", 0.1, TimeoutError),
        ]:
            with tempfile.TemporaryDirectory() as directory:
                resources = Resources(Path(directory))
                try:
                    with self.assertRaises(expected):
                        resources.run([sys.executable, "-c", code], "child", timeout=timeout)
                finally:
                    self.assertEqual(resources.close(), [])
                for process in resources.processes:
                    self.assertIsNotNone(process.poll())
                    with self.assertRaises(ProcessLookupError):
                        os.killpg(process.pid, signal.SIGCONT)

    def test_cleanup_failure_is_reportable(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Resources(Path(directory))
            resources.container = "only-this-test-container"
            with patch.object(resources, "docker", side_effect=RuntimeError("failed")) as docker:
                self.assertEqual(resources.close(), ["database container cleanup failed"])
                docker.assert_called_once_with("rm", "--force", "--volumes", "only-this-test-container")

    def test_background_output_is_bounded_and_fails_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Resources(Path(directory))
            with patch("run.MAX_LOG", 1024):
                process, log = resources.start(
                    [sys.executable, "-c", "import sys; sys.stdout.write('x' * 100000)"], "noisy")
                process.wait(timeout=5)
                errors = resources.close()
            self.assertIn("noisy: output limit exceeded", errors)
            self.assertLessEqual(log.stat().st_size, 1024)

    def test_settlement_rejects_wrong_cost_and_balance(self):
        log = {
            "billed_at": "2026-09-16T00:00:00Z", "outcome": "succeeded",
            "response_status_code": 200, "client_model": "e2e-client",
            "upstream_model": "e2e-wire", "channel_id": "channel",
            "request_protocol": "sse", "api_operation": "responses",
            "input_tokens": 5, "output_tokens": 2, "cost_amount": "0.000009",
        }
        data = {"console": "http://localhost", "token": "test", "api_key_id": "key", "channel_id": "channel"}
        for cost, balance, expected in [
            ("0", "99.999991", "cost mismatch"),
            ("0.000009", "100", "balance mismatch"),
        ]:
            def fake_request(_base, path, **_kwargs):
                if "request-logs" in path:
                    return [{**log, "cost_amount": cost}], {}
                return {"balance_amount": balance}, {}

            with patch("run.request", side_effect=fake_request):
                with self.assertRaisesRegex(RuntimeError, expected):
                    verify_settlement(data, 1)


if __name__ == "__main__":
    unittest.main()
