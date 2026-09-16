"""Bounded loopback Responses fixture; no forwarding or external network calls."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

CONTRACT = json.loads((Path(__file__).parent / "scenarios/responses.json").read_text())
EVENTS = json.loads((Path(__file__).parent / "scenarios/response-events.json").read_text())
MAX_BODY = 1024 * 1024
MAX_REQUESTS = 8


def response_events(response_id, item):
    response = {
        "id": response_id, "object": "response", "model": CONTRACT["wire_model"],
        "status": "completed", "output": [item], "usage": CONTRACT["usage"],
    }
    part = item.get("content", [{}])[0]
    variables = {
        "$response": response, "$item": item, "$item_id": item["id"],
        "$partial_response": {**response, "status": "in_progress", "output": [], "usage": None},
        "$partial_item": {**item, "status": "in_progress",
                          **({"arguments": ""} if item["type"] == "function_call" else {"content": []})},
        "$arguments": item.get("arguments"), "$text": part.get("text"), "$part": part,
        "$partial_part": {**part, "text": ""},
    }

    def render(value):
        if isinstance(value, str) and value.startswith("$"):
            return variables[value]
        if isinstance(value, dict):
            return {key: render(child) for key, child in value.items()}
        if isinstance(value, list):
            return [render(child) for child in value]
        return value

    events = render(EVENTS["prefix"] + EVENTS[item["type"]] + EVENTS["suffix"])
    return [{**event, "sequence_number": i} for i, event in enumerate(events)], response


def encode_sse(events):
    return [f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode() for event in events]


class Scenario:
    def __init__(self, marker):
        self.marker = marker
        self.evidence = []
        self.errors = []
        self.tool_issued = False
        self.tool_completed = False
        self.requests = 0
        self.lock = threading.Lock()

    def respond(self, body, transport="http", connection=None):
        with self.lock:
            try:
                return self._respond(body, transport, connection)
            except (ValueError, KeyError, TypeError) as error:
                if len(self.errors) < MAX_REQUESTS:
                    self.errors.append(str(error))
                raise ValueError(str(error)) from error

    def _respond(self, body, transport, connection):
        self.requests += 1
        if self.requests > MAX_REQUESTS:
            raise ValueError("request limit exceeded")
        if body.get("model") != CONTRACT["wire_model"]:
            raise ValueError("candidate model rewrite missing")
        tools = body.get("tools", [])
        mode = "responses-tool-cycle" if tools else "responses-text"
        scenario = CONTRACT["scenarios"][mode]
        index = len(self.evidence)
        output_verified = False
        if tools and not self.tool_issued:
            names = {tool.get("name") for tool in tools if tool.get("type") == "function"}
            command = f"cat {scenario['marker_file']}"
            if "exec_command" in names:
                name, arguments = "exec_command", {"cmd": command, "max_output_tokens": 1000}
            elif "shell_command" in names:
                name, arguments = "shell_command", {"command": command}
            elif "shell" in names:
                name, arguments = "shell", {"command": ["cat", scenario["marker_file"]]}
            elif "read" in names:
                name, arguments = "read", {"path": scenario["marker_file"]}
            else:
                raise ValueError("client did not advertise a supported shell tool")
            item = {
                "id": "fc_system_e2e", "type": "function_call", "status": "completed",
                "call_id": scenario["call_id"], "name": name,
                "arguments": json.dumps(arguments),
            }
            self.tool_issued = True
        else:
            if tools:
                if self.tool_completed:
                    raise ValueError("unexpected third tool-cycle request")
                outputs = [
                    entry for entry in body.get("input", [])
                    if isinstance(entry, dict) and entry.get("type") == "function_call_output"
                    and entry.get("call_id") == scenario["call_id"]
                ]
                # The marker is random and absent from the prompt: matching final
                # prose cannot substitute for evidence of actual file execution.
                if len(outputs) != 1 or self.marker not in str(outputs[0].get("output", "")):
                    raise ValueError("matching successful tool output missing")
                self.tool_completed = output_verified = True
            item = {
                "id": f"msg_e2e_{index}", "type": "message", "role": "assistant",
                "status": "completed", "content": [
                    {"type": "output_text", "text": scenario["final_text"], "annotations": []}
                ],
            }
        self.evidence.append({
            "scenario": mode, "model": body["model"], "stream": body.get("stream", False),
            "output_type": item["type"], "tool_output_verified": output_verified,
            "transport": transport, "connection": connection,
            "previous_response_id": body.get("previous_response_id"),
        })
        return response_events(f"resp_e2e_{index}", item)


class Upstream:
    def __init__(self, marker, response_gate=None):
        self.scenario = Scenario(marker)
        scenario = self.scenario

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                try:
                    if self.path != "/v1/responses":
                        raise ValueError("unexpected upstream path")
                    if self.headers.get("Transfer-Encoding"):
                        raise ValueError("chunked fixture requests are unsupported")
                    length = int(self.headers.get("Content-Length", "0"))
                    if not 0 < length <= MAX_BODY:
                        raise ValueError("invalid fixture body length")
                    self.connection.settimeout(10)
                    body = json.loads(self.rfile.read(length))
                    events, response = scenario.respond(body)
                    if response_gate is not None and not response_gate.wait(timeout=10):
                        raise ValueError("response gate timed out")
                    events = encode_sse(events)
                    payload = b"".join(events) if body.get("stream") else json.dumps(response).encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream" if body.get("stream") else "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    if body.get("stream"):
                        for event in events:
                            self.wfile.write(event)
                            self.wfile.flush()
                    else:
                        self.wfile.write(payload)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except (ValueError, TypeError, KeyError) as error:
                    with scenario.lock:
                        if len(scenario.errors) < MAX_REQUESTS:
                            scenario.errors.append(str(error))
                    self.send_error(400, "fixture contract failed")

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.server_port}"

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
