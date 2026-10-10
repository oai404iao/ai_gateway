# Shared protocol scenarios

> Status: Current. Loopback Responses fixtures shared across system and Rust tests.

`scenarios/responses.json` owns stable scenario IDs, deterministic usage,
terminal text and the tool-cycle contract. `scenarios/response-events.json` owns
wire-event templates consumed by Python HTTP/SSE/WS adapters and
`tests/websocket_integration.rs`. Variables substitute JSON values, not source code.
`upstream.py` handles HTTP/SSE; `websocket.py` uses a hash-pinned `websockets`
dependency from `e2e/requirements.txt`. Neither calls external services.

- `responses-text`: one JSON response, used after the browser changes routing.
- `responses-tool-cycle`: two generations separated by an actual Codex shell
  or Pi read call. A per-run random marker must return under the emitted call ID.
- WS keeps previous response IDs connection-local; warmup cannot satisfy tool
  coverage, and continuation on another socket or without its ID fails.
- Wrong model aliases, missing/mismatched tool output and extra tool rounds fail.
- The adapter bounds request bodies, request count and retained error summaries.
- Artifacts retain structural evidence, not Headers or raw request bodies.

This is not a complete OpenAI emulator. There are no images or cross-format
conversion. Existing Rust upstream mocks,
Console MSW/Playwright handlers and the performance Mock remain separate.
Future adapters should share scenario contracts where useful, not become one
universal server. Scenario oracles include wrong call IDs, missing tool output,
wrong continuation, errors and missing terminals. See [system E2E](../docs/development/system-e2e.md).

`response_adapter.py` is a separate bounded loopback fixture for native response
adapter acceptance. It emits Chat and Responses JSON/SSE with raw 5/2 usage;
Chat SSE can deliberately omit `[DONE]`, interleave two requests at content-event
boundaries, or hold a dispatched request while Console settings change. Responses
SSE emits completed, incomplete or cancelled terminal events with matching status,
raw usage, and no explicit error fields. It never
adapts the content itself: independent harness oracles require the installed
plugin to modify text while preserving usage and terminal semantics. It does
not emulate WebSocket or cross-format conversion.

`usage_normalization.py` emits unchanged OpenAI-compatible JSON/SSE envelopes
with explicit OpenAI inclusive-input or Anthropic exclusive-input usage dialects.
Both represent canonical `5/3/0/2/0` counters. It bounds bodies and requests,
records structural evidence only, and performs neither parsing normalization
nor response adaptation. This synthetic fixture is not an Anthropic transport
implementation; installed connector settings select the pure parser.
