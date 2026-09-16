# Shared protocol scenarios

> Status: Current. Initial loopback Responses fixtures for system E2E.

`scenarios/responses.json` owns stable scenario IDs, deterministic usage,
terminal text and the tool-cycle contract. `upstream.py` is a bounded
standard-library HTTP/SSE adapter, imported by `e2e/run.py` and its offline tests.
It never calls external services and binds only to loopback.

- `responses-text`: one JSON response, used after the browser changes routing.
- `responses-tool-cycle`: two SSE generations separated by an actual CLI shell
  read. A per-run random marker must return under the emitted call ID.
- Wrong model aliases, missing/mismatched tool output and extra tool rounds fail.
- The adapter bounds request bodies, request count and retained error summaries.
- Artifacts retain structural evidence, not Headers or raw request bodies.

This is not a complete OpenAI emulator. There is no WS, images, cross-format
conversion or arbitrary failure injection yet. Existing Rust upstream mocks,
Console MSW/Playwright handlers and the performance Mock remain separate.
Future adapters should share scenario contracts where useful, not become one
universal server. See [system E2E](../docs/development/system-e2e.md).
