"""Bounded OpenAI-compatible envelope fixture with explicit synthetic usage dialects."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MAX_REQUESTS = 10
MAX_BODY = 65536
OPENAI_USAGE = {
    "input_tokens": 5, "output_tokens": 2, "total_tokens": 7,
    "input_tokens_details": {"cached_tokens": 3},
    "output_tokens_details": {"reasoning_tokens": 0},
}
ANTHROPIC_USAGE = {
    "input_tokens": 2, "cache_read_input_tokens": 3,
    "cache_creation_input_tokens": 0, "output_tokens": 2,
}


def fixture_payload(profile, stream):
    if profile not in ("openai", "anthropic", "custom"):
        raise ValueError("unsupported fixture usage profile")
    response = {
        "id": "resp_usage", "object": "response", "model": "usage-wire",
        "status": "completed", "output": [{
            "id": "msg_usage", "type": "message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "USAGE_OK", "annotations": []}],
        }],
        "usage": OPENAI_USAGE if profile == "openai" else ANTHROPIC_USAGE,
    }
    if not stream:
        return json.dumps(response, indent=2).encode() + b"\n"
    events = [
        {"type": "response.created", "response": {
            **response, "status": "in_progress", "output": [], "usage": None}},
        {"type": "response.output_text.delta", "item_id": "msg_usage",
         "output_index": 0, "content_index": 0, "delta": "USAGE_OK"},
        {"type": "response.completed", "response": response},
    ]
    return "".join(
        f"event: {event['type']}\nid: usage-{index}\ndata: "
        f"{json.dumps({**event, 'sequence_number': index})}\n\n"
        for index, event in enumerate(events)
    ).encode()


class UsageUpstream:
    def __init__(self):
        self.profile = "openai"
        self.evidence = []
        self.errors = []
        self.lock = threading.Lock()
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                try:
                    self.connection.settimeout(10)
                    length = int(self.headers.get("Content-Length", "0"))
                    if self.path != "/v1/responses" or not 0 < length <= MAX_BODY:
                        raise ValueError("unexpected fixture path or body size")
                    body = json.loads(self.rfile.read(length))
                    if body.get("model") != "usage-wire":
                        raise ValueError("candidate model rewrite missing")
                    stream = body.get("stream", False)
                    with fixture.lock:
                        if len(fixture.evidence) >= MAX_REQUESTS:
                            raise ValueError("request limit exceeded")
                        profile = fixture.profile
                        fixture.evidence.append({"profile": profile, "stream": stream,
                                                 "model": body["model"]})
                    payload = fixture_payload(profile, stream)
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream" if stream else "application/json")
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except (ValueError, TypeError, KeyError) as error:
                    with fixture.lock:
                        if len(fixture.errors) < MAX_REQUESTS:
                            fixture.errors.append(str(error))
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
