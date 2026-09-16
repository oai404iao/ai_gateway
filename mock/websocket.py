"""Responses WS fixture with strict connection-local continuation evidence."""

import json
import threading
from websockets.exceptions import ConnectionClosed
from websockets.sync.server import serve

from mock.upstream import CONTRACT, MAX_BODY, Scenario


class WebSocketUpstream:
    def __init__(self, marker):
        self.scenario = Scenario(marker)
        self.connections = set()
        self.lock = threading.Lock()
        self.connection_count = 0
        self.warmups = 0
        self.errors = []
        self.server = serve(
            self.handle, "127.0.0.1", 0, compression=None,
            open_timeout=5, close_timeout=1, max_size=MAX_BODY, max_queue=4,
        )
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server.socket.getsockname()[1]}"

    def handle(self, socket):
        with self.lock:
            self.connection_count += 1
            connection = self.connection_count
            self.connections.add(socket)
        try:
            if connection > 2 or socket.request.path != "/v1/responses":
                raise ValueError("unexpected WS connection or path")
            previous = None
            for _ in range(4):
                body = json.loads(socket.recv(timeout=30))
                if body.get("type") != "response.create" or body.get("model") != CONTRACT["wire_model"]:
                    raise ValueError("invalid WS create/model")
                if body.get("previous_response_id") and body["previous_response_id"] != previous:
                    raise ValueError("continuation moved to another socket or response")
                if body.get("generate") is False:
                    if self.warmups:
                        raise ValueError("duplicate warmup")
                    self.warmups += 1
                    previous = "resp_e2e_warmup"
                    response = {
                        "id": previous, "object": "response", "model": CONTRACT["wire_model"],
                        "status": "completed", "output": [],
                        "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                    }
                    socket.send(json.dumps({"type": "response.completed", "response": response}))
                    continue
                if self.scenario.tool_issued and not body.get("previous_response_id"):
                    raise ValueError("tool continuation must use previous_response_id")
                events, response = self.scenario.respond(body, "websocket", connection)
                for event in events:
                    socket.send(json.dumps(event))
                previous = response["id"]
        except ConnectionClosed:
            pass
        except (ValueError, TimeoutError, KeyError, TypeError) as error:
            with self.lock:
                if len(self.errors) < 8:
                    self.errors.append(str(error))
            socket.close(code=1008, reason="fixture contract failed")
        finally:
            with self.lock:
                self.connections.discard(socket)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        with self.lock:
            connections = list(self.connections)
        for socket in connections:
            socket.close()
        self.thread.join(timeout=5)
