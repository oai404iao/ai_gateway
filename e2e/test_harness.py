"""Offline contract and lifecycle tests; never start Docker or paid upstreams."""

import json
import io
import hashlib
import os
from pathlib import Path
import signal
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch
from urllib.error import HTTPError

from run import (
    Resources, exercise_codex_oauth, exercise_plugin_lifecycle, load_plugin_fixture, plugin_toml, redact,
    verify_oauth_plan, verify_settlement,
)
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
    def test_http_error_codes_accept_console_and_data_plane_shapes_without_echoing(self):
        for body in (
            {"error": "codex_oauth_state_mismatch"},
            {"error": {"code": "codex_oauth_state_mismatch"}},
        ):
            error = HTTPError("http://localhost", 422, "rejected", {},
                              io.BytesIO(json.dumps(body).encode()))
            with patch("run.HTTP.open", side_effect=error):
                with self.assertRaisesRegex(RuntimeError, "codex_oauth_state_mismatch"):
                    request("http://localhost", "/test")
        error = HTTPError("http://localhost", 422, "rejected", {},
                          io.BytesIO(b'{"error":"echoed private request"}'))
        with patch("run.HTTP.open", side_effect=error):
            with self.assertRaisesRegex(RuntimeError, r"\(unknown\)"):
                request("http://localhost", "/test")

    def test_plugin_fixture_is_readonly_and_directory_configuration_is_explicit(self):
        with self.assertRaisesRegex(RuntimeError, "AI_GATEWAY_TEST_CODEX_PLUGIN"):
            load_plugin_fixture(None)
        with self.assertRaisesRegex(RuntimeError, "absolute"):
            load_plugin_fixture("relative.so")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'fixture"quoted.so'
            path.write_bytes(b"synthetic native fixture")
            with self.assertRaisesRegex(RuntimeError, "readonly"):
                load_plugin_fixture(str(path))
            path.chmod(0o444)
            plugin = load_plugin_fixture(str(path))
            self.assertEqual(plugin["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())
            plugin_directory = Path(directory) / 'plugins"quoted'
            self.assertEqual(tomllib.loads(plugin_toml(plugin_directory)),
                             {"plugins": {"directory": str(plugin_directory)}})
            link = Path(directory) / "linked.so"
            link.symlink_to(path)
            with self.assertRaisesRegex(RuntimeError, "symlinks"):
                load_plugin_fixture(str(link))

    def test_codex_oauth_requires_provider_plan_and_rejects_state_without_exchange(self):
        flow = {
            "flow_id": "synthetic-flow",
            "expires_at": "2026-09-16T00:15:00Z",
            "authorization_url": "https://auth.openai.com/oauth/authorize?"
            "response_type=code&code_challenge_method=S256&"
            "redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&"
            f"state={'s' * 43}&code_challenge={'c' * 43}",
        }
        verify_oauth_plan(flow)
        for url in (
            flow["authorization_url"].replace("auth.openai.com", "unexpected.test"),
            flow["authorization_url"].replace("S256", "plain"),
            flow["authorization_url"].replace("code_challenge=", "missing="),
        ):
            with self.assertRaises(RuntimeError):
                verify_oauth_plan({**flow, "authorization_url": url})
        data = {"console": "http://localhost", "token": "synthetic"}
        with patch("run.request", side_effect=[
            (flow, {}), RuntimeError("HTTP 422 (codex_oauth_state_mismatch)"),
        ]) as api:
            self.assertEqual(exercise_codex_oauth(data)["status"], "passed")
            self.assertEqual(api.call_count, 2)
            self.assertIn("incorrect-synthetic-state", api.call_args.args[3]["callback_url"])
        for completion in (({}, {}), RuntimeError("HTTP 502 (codex_upstream_unavailable)")):
            with patch("run.request", side_effect=[(flow, {}), completion]):
                with self.assertRaises(RuntimeError):
                    exercise_codex_oauth(data)

    def test_plugin_lifecycle_oracle_rejects_automatic_enable_stale_writes_and_settings_loss(self):
        for fault in (None, "auto_enable", "stale_write", "reset_settings", "host_settings"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                (Path(directory) / "codex-test.tar.gz").write_bytes(b"synthetic-package")
                plugin = {"path": str(Path(directory) / "fixture.so"), "sha256": "a" * 64}
                state = {"enabled": False, "revision": 0, "values": {"client_version": "original"}}
                authorization = None

                def api(_base, path, method="GET", body=None, token=None, etag=None,
                        plugin_authorization=None):
                    nonlocal authorization
                    headers = {"ETag": f'"{state["revision"]}"'}
                    if path.endswith("/reauth"):
                        authorization = object()
                        return {"token": authorization}, {}
                    if method != "GET":
                        self.assertIs(plugin_authorization, authorization)
                        self.assertIsNotNone(authorization)
                        authorization = None
                    if path.endswith("/install"):
                        self.assertEqual(body, b"synthetic-package")
                        state["enabled"] = fault == "auto_enable"
                        return {"id": "install-job"}, {}
                    if "/jobs/" in path:
                        return {"status": "succeeded"}, {}
                    if path.endswith("/system/connectors"):
                        return ([{"id": "codex"}] if state["enabled"] else []), {}
                    if path.endswith("/system/settings"):
                        return ({"codex": {}} if fault == "host_settings" else {}), {}
                    if method == "PUT":
                        if etag != headers["ETag"] and fault != "stale_write":
                            raise RuntimeError("HTTP 409 (conflict)")
                        if path.endswith("/state"):
                            state["enabled"] = body["enabled"]
                            if fault == "reset_settings" and state["revision"] > 1 and body["enabled"]:
                                state["values"] = {"client_version": "original"}
                        else:
                            state["values"] = dict(body["values"])
                        state["revision"] += 1
                        return {}, {}
                    if path.endswith("/settings"):
                        return {"schema_version": 1, "values": dict(state["values"])}, headers
                    return {"enabled": state["enabled"], "artifacts": [{"digest": plugin["sha256"]}]}, headers

                data = {"console": "http://localhost", "token": "synthetic", "password": "synthetic"}
                with patch("run.request", side_effect=api):
                    if fault is None:
                        self.assertEqual(exercise_plugin_lifecycle(data, plugin)["status"], "passed")
                    else:
                        with self.assertRaises(RuntimeError):
                            exercise_plugin_lifecycle(data, plugin)

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
