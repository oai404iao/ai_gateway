# Project system E2E

> Status: Current. Covers a real browser, pinned Codex HTTP/WS and Pi clients,
> crash/replay, and durable admission fault acceptance against the production
> gateway and disposable PostgreSQL or SQLite, including offline paired SQLite recovery.

See [the maintainer guide](../docs/development/system-e2e.md) for setup,
scenarios, isolation, safe reports, CI, and remaining phase-one work.

```bash
python3 -m venv target/e2e-venv
target/e2e-venv/bin/pip install --require-hashes -r e2e/requirements.txt
target/e2e-venv/bin/python -m unittest discover -s e2e -p 'test_*.py'
node --test e2e/browser-checks.test.mjs
export AI_GATEWAY_TEST_CODEX_PLUGIN="$(scripts/prepare-connector-tests.sh)"
target/e2e-venv/bin/python e2e/run.py
target/e2e-venv/bin/python e2e/run.py --backend sqlite
```

The runner requires a built embedded-Console binary, installed Console
dependencies/Chromium, Docker (PostgreSQL only), OpenSSL, a C compiler, Node, Python 3.11+, Linux,
and the exact client versions in `clients.json`. It does not install dependencies or build implicitly.
The fixture-build exceptions are trusted local SDK examples. Preflight builds
`response_adapter` in `target/response-adapter-tests-build`, packages
it under `target/response-adapter-tests/<digest>`, and validates its identity
and digest before uploading through the real Console plugin API. Set
`AI_GATEWAY_TEST_RESPONSE_PLUGIN` to an absolute readonly library path with its
companion `example-response-adapter-test.tar.gz` to reuse an explicitly built
fixture from `bash scripts/prepare-response-adapter-tests.sh`. Missing or invalid
fixtures fail, rather than silently skipping scenarios.
It also builds `usage_parser` in `target/usage-parser-tests-build` and packages
it under `target/usage-parser-tests/<digest>`. Set `AI_GATEWAY_TEST_USAGE_PLUGIN`
to an absolute readonly library path plus companion `example-usage-parser-test.tar.gz`
to reuse `bash scripts/prepare-usage-parser-tests.sh` output. Its manifest must
advertise pure usage parsing and must not advertise response adaptation commands.
It never reads local gateway configuration or real-upstream credentials.
Build with `--features sqlite-backend,embedded-console-ui` for the two-backend matrix.
The external Codex test plugin is pinned by `tests/fixtures/codex-plugin.json`.
The harness requires its explicit readonly path and companion `codex-test.tar.gz`
package, configures a private `[plugins].directory`, and records the digest.
Missing fixtures fail preflight. The lifecycle scenario uploads the package,
reauthenticates each write, enables it, changes independent settings, rejects
stale settings writes, and disables/re-enables without resetting settings.
The embedded browser repeats package upload, edits the plugin-generated form
with its real ETag, and verifies persisted settings and enable/disable controls.
The `codex-plugin-oauth-plan` scenario checks provider authorization planning and
host callback-state rejection without contacting the provider. Successful OAuth
exchange, refresh/quota and Codex-channel forwarding remain deterministic Rust
integration coverage; the real CLI tool-cycle scenarios use ordinary channels.

The `plugin-response-*` scenarios run after the original durability and SQLite
backup checks, using an independent synthetic API key and a balance baseline.
They cover same-format Chat and Responses JSON/SSE text adaptation,
raw usage and settlement, unsupported-protocol rejection before dispatch,
interleaved request-local SSE state, in-flight settings pinning, malformed
JSON/SSE and metering tamper rejection, and truncated SSE without a synthesized
terminal. Responses SSE preserves completed, incomplete and cancelled terminals
without synthesizing `response.completed` or `[DONE]`; incomplete/cancelled
requests retain raw 5/2 usage and log `upstream_sse_error`, not an adapter failure.
Failed requests retain available raw usage but follow the ordinary
zero-cost failure policy. Reports count these thirteen dispatched durable requests
separately from the existing settlement results; the protocol-denied request
uses another key and must neither dispatch nor consume quota.
Adapter coverage does not include WebSocket adaptation, cross-format conversion,
or an adapter-inclusive SQLite backup.

The five `plugin-usage-*` scenarios run after adapter settlement with another key
and balance baseline. OpenAI and Anthropic usage profiles each run JSON/SSE;
the custom pure parser runs JSON. All compare exact upstream response bytes and
canonical input/cache-read/cache-write/output/reasoning counters `5/3/0/2/0`,
then verify durable logs, `0.000006` per-request cost, balance and quota.
The fixture uses a synthetic OpenAI-compatible envelope with different usage
dialects, not an actual Anthropic HTTP route or cross-format conversion.
Invalid custom counters and unknown usage are native/offline negative tests,
not unpriced successful requests added to the full suite. Reports keep
`usage_settlement` separate and add each suite's verified durable count once.

Do not replace the fast mocked UI suite under `web/console/e2e/`, backend
integration tests, paid opt-in smoke, or performance tooling with this suite.
