# Shared protocol scenarios

> Status: Current. Deterministic loopback fixtures for system and Rust tests.

Use these fixtures to exercise protocol behavior without external services.
They share scenario contracts, not one universal OpenAI emulator. The system
runner owns startup and isolation; see [system E2E](../docs/development/system-e2e.md).
Existing Rust mocks, Console MSW/Playwright handlers and the performance Mock
remain separate.

## Choose the fixture

| File | Responsibility |
| --- | --- |
| [`scenarios/responses.json`](scenarios/responses.json) | Stable scenario IDs, deterministic usage, terminal text and tool-cycle contract |
| [`scenarios/response-events.json`](scenarios/response-events.json) | Wire-event templates shared by Python adapters and Rust WebSocket tests |
| [`upstream.py`](upstream.py) | HTTP JSON/SSE Responses scenarios |
| [`websocket.py`](websocket.py) | Connection-local Responses WebSocket scenarios; uses the hash-pinned dependency in `e2e/requirements.txt` |
| [`response_adapter.py`](response_adapter.py) | Raw Chat/Responses JSON/SSE inputs for native adapter acceptance |
| [`usage_normalization.py`](usage_normalization.py) | Unchanged envelopes with explicitly selected usage dialects |

Template variables substitute JSON values, not source code. Keep scenario
semantics in the shared contracts; do not independently redefine the same
oracle in each transport adapter.

## Preserve scenario invariants

- `responses-text` returns one JSON response after the browser changes routing.
- `responses-tool-cycle` requires two generations separated by an actual Codex
  shell or Pi read call. A per-run random marker must return under the emitted
  call ID; warmup cannot satisfy tool coverage.
- WebSocket continuation IDs are connection-local. A continuation on another
  socket or without the required ID fails.
- Wrong aliases, wrong call IDs, missing/mismatched tool output, extra tool
  rounds, errors and missing terminals must fail their respective oracles.
- Adapters bound bodies, request counts and retained error summaries. Reports
  retain structural evidence, not headers or raw request bodies.

## Test response adaptation without doing it in the mock

`response_adapter.py` emits raw 5/2 usage. Chat SSE can omit `[DONE]`, interleave
requests at content-event boundaries, or hold a dispatched request while
Console settings change. Responses SSE emits completed, incomplete or cancelled
terminals with matching status, raw usage and no explicit error fields.

The fixture never adapts content itself. Independent harness oracles require
the installed plugin to change text without changing usage or terminal meaning.
There is no WebSocket adaptation or cross-format conversion here.

## Test usage selection without normalizing in the mock

`usage_normalization.py` emits OpenAI-compatible JSON/SSE envelopes with either
OpenAI inclusive-input or Anthropic exclusive-input usage. Both represent
canonical `5/3/0/2/0` counters. The fixture bounds requests/bodies and retains
structural evidence only; it performs neither normalization nor adaptation.

Installed connector settings select the pure parser. The synthetic Anthropic
usage dialect does not imply an Anthropic HTTP route. Images and a full
OpenAI emulator are outside this fixture set's scope.
