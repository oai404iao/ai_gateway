"""Negative protocol oracles and scoped syscall injection, without a gateway."""

import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

from websockets.exceptions import ConnectionClosedError
from websockets.sync.client import connect

from faults import build_injector
from mock.websocket import WebSocketUpstream
from run import Resources, validate_pi_output


def create(**extra):
    return {
        "type": "response.create", "model": "e2e-wire",
        "tools": [{"type": "function", "name": "exec_command"}], **extra,
    }


def generate(socket, body):
    socket.send(json.dumps(body))
    while True:
        event = json.loads(socket.recv(timeout=5))
        if event["type"] == "response.completed":
            return event["response"]


class WebSocketTests(unittest.TestCase):
    def test_warmup_does_not_count_as_tool_success(self):
        with WebSocketUpstream("marker") as upstream:
            with connect(upstream.url.replace("http:", "ws:") + "/v1/responses", proxy=None) as socket:
                response = generate(socket, create(generate=False))
                self.assertEqual(response["usage"]["input_tokens"], 0)
                self.assertEqual(upstream.scenario.evidence, [])
                self.assertFalse(upstream.scenario.tool_completed)
                response = generate(socket, create(previous_response_id=response["id"]))
                final = generate(socket, create(previous_response_id=response["id"], input=[{
                    "type": "function_call_output", "call_id": "call_system_e2e", "output": "marker",
                }]))
                self.assertEqual(final["output"][0]["content"][0]["text"], "E2E_TOOL_OK")
                self.assertTrue(upstream.scenario.tool_completed)
                self.assertEqual(upstream.errors, [])

    def test_missing_or_wrong_continuation_is_rejected(self):
        for previous in (None, "resp_wrong"):
            with WebSocketUpstream("marker") as upstream:
                with connect(upstream.url.replace("http:", "ws:") + "/v1/responses", proxy=None) as socket:
                    generate(socket, create())
                    body = create(input=[{
                        "type": "function_call_output", "call_id": "call_system_e2e", "output": "marker",
                    }])
                    if previous:
                        body["previous_response_id"] = previous
                    with self.assertRaises(ConnectionClosedError):
                        generate(socket, body)
                self.assertTrue(upstream.errors)
                self.assertFalse(upstream.scenario.tool_completed)

    def test_connection_local_state_cannot_move(self):
        with WebSocketUpstream("marker") as upstream:
            url = upstream.url.replace("http:", "ws:") + "/v1/responses"
            with connect(url, proxy=None) as first:
                response = generate(first, create())
            with connect(url, proxy=None) as second:
                with self.assertRaises(ConnectionClosedError):
                    generate(second, create(previous_response_id=response["id"]))
            self.assertIn("another socket", upstream.errors[0])


class PiOracleTests(unittest.TestCase):
    def test_rejects_wrong_tool_id_error_missing_terminal_or_marker(self):
        events = [
            {"type": "tool_execution_end", "isError": False, "toolName": "read",
             "toolCallId": "call_system_e2e|fc_system_e2e", "result": {"content": "marker"}},
            {"type": "message_end", "message": {
                "role": "assistant", "stopReason": "stop",
                "content": [{"type": "text", "text": "E2E_TOOL_OK"}],
            }},
            {"type": "agent_end"},
        ]
        encode = lambda entries: "\n".join(json.dumps(entry) for entry in entries)
        validate_pi_output(encode(events), "marker")
        bad = []
        for key, value in (("toolCallId", "wrong"), ("isError", True), ("result", {})):
            altered = copy.deepcopy(events)
            altered[0][key] = value
            bad.append(altered)
        bad.extend([events[:-1], events[1:]])
        for altered in bad:
            with self.assertRaises(RuntimeError):
                validate_pi_output(encode(altered), "marker")


class FaultInjectorTests(unittest.TestCase):
    def test_only_selected_descriptor_fails_with_selected_errno(self):
        with tempfile.TemporaryDirectory() as directory:
            resources = Resources(Path(directory))
            try:
                library = build_injector(resources)
                target = Path(directory) / "events.log"
                target.touch()
                control = Path(directory) / "control"
                for mode, number in (("ENOSPC", 28), ("EACCES", 13)):
                    control.write_text(mode)
                    code = (
                        "import os,errno\n"
                        f"fd=os.open({str(target)!r},os.O_WRONLY)\n"
                        "try:\n"
                        " os.write(fd,b'not-written')\n"
                        " raise AssertionError('injection missing')\n"
                        "except OSError as error:\n"
                        f" assert error.errno=={number}\n"
                        "finally: os.close(fd)\n"
                        f"fd=os.open({str(target.with_name('unrelated'))!r},os.O_WRONLY|os.O_CREAT,0o600)\n"
                        "os.write(fd,b'allowed');os.close(fd)\n"
                    )
                    resources.run([sys.executable, "-c", code], mode, env={
                        **resources.env, "LD_PRELOAD": str(library),
                        "E2E_SPOOL_FAULT_PATH": str(target), "E2E_SPOOL_FAULT_CONTROL": str(control),
                    })
                    self.assertEqual(target.read_bytes(), b"")
                    self.assertEqual(target.with_name("unrelated").read_bytes(), b"allowed")
            finally:
                self.assertEqual(resources.close(), [])
