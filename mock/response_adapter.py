"""Bounded Chat/Responses fixture for same-format native response adaptation."""

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

USAGE = {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
MAX_REQUESTS = 16
MAX_BODY = 65536


def chat_json():
    return {
        "id": "chat_adapter", "object": "chat.completion", "created": 1,
        "model": "adapter-wire",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "raw-json"},
                     "finish_reason": "stop"}],
        "usage": dict(USAGE),
    }


def responses_json():
    return {
        "id": "resp_adapter", "object": "response", "model": "adapter-wire",
        "status": "completed", "output": [{
            "id": "msg_adapter", "type": "message", "role": "assistant", "status": "completed",
            "content": [{"type": "output_text", "text": "raw-responses", "annotations": []}],
        }],
        "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7},
    }


def responses_events(terminal="completed"):
    if terminal not in ("completed", "incomplete", "cancelled"):
        raise ValueError("invalid Responses terminal")
    response = {**responses_json(), "status": terminal}
    events = [{
        "type": "response.created",
        "response": {**response, "status": "in_progress", "output": [], "usage": None},
    }]
    events.extend({
        "type": "response.output_text.delta", "output_index": 0, "content_index": 0,
        "item_id": "msg_adapter", "delta": text,
    } for text in ("raw-first", "raw-second"))
    events.append({"type": "response." + terminal, "response": response})
    return [{**event, "sequence_number": index} for index, event in enumerate(events)]


def responses_sse(terminal="completed"):
    return [f"event: {event['type']}\ndata: {json.dumps(event)}\n\n".encode()
            for event in responses_events(terminal)]


def chat_events(truncated=False):
    base = {"id": "chat_adapter", "object": "chat.completion.chunk",
            "created": 1, "model": "adapter-wire"}
    events = [
        {**base, "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": None}]}
        for text in ("raw-first", "raw-second")
    ]
    events.append({**base, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
    events.append({**base, "choices": [], "usage": dict(USAGE)})
    return [json.dumps(value) for value in events] + ([] if truncated else ["[DONE]"])


class AdapterUpstream:
    def __init__(self):
        self.evidence = []
        self.errors = []
        self.lock = threading.Lock()
        self.arrived = threading.Event()
        self.release = threading.Event()
        self.release.set()
        self.interleave = None
        self.truncated = False
        self.responses_terminal = "completed"
        fixture = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                try:
                    self.connection.settimeout(10)
                    length = int(self.headers.get("Content-Length", "0"))
                    if (self.path not in ("/v1/chat/completions", "/v1/responses")
                            or not 0 < length <= MAX_BODY):
                        raise ValueError("unexpected path or body size")
                    body = json.loads(self.rfile.read(length))
                    if body.get("model") != "adapter-wire":
                        raise ValueError("candidate model rewrite missing")
                    with fixture.lock:
                        if len(fixture.evidence) >= MAX_REQUESTS:
                            raise ValueError("request limit exceeded")
                        fixture.evidence.append({"path": self.path, "stream": body.get("stream", False),
                                                 "model": body["model"]})
                    fixture.arrived.set()
                    if not fixture.release.wait(timeout=10):
                        raise ValueError("response gate timeout")
                    stream = body.get("stream", False)
                    if stream and self.path == "/v1/responses":
                        payloads = responses_sse(fixture.responses_terminal)
                    elif stream:
                        payloads = [f"data: {value}\n\n".encode()
                                    for value in chat_events(fixture.truncated)]
                    else:
                        payloads = [json.dumps(responses_json() if self.path == "/v1/responses"
                                               else chat_json()).encode()]
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream" if stream else "application/json")
                    self.send_header("Content-Length", str(sum(map(len, payloads))))
                    self.end_headers()
                    for index, payload in enumerate(payloads):
                        self.wfile.write(payload)
                        self.wfile.flush()
                        if stream and index < 2 and fixture.interleave:
                            fixture.interleave.wait(timeout=10)
                except (BrokenPipeError, ConnectionResetError, threading.BrokenBarrierError):
                    pass
                except (ValueError, TypeError, KeyError) as error:
                    with fixture.lock:
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
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
