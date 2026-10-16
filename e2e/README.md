# Run the system E2E suite

> Status: Current. Real browser/CLI acceptance with isolated synthetic state.

Use this suite to test the production gateway, embedded Console, pinned Codex
HTTP/WebSocket and Pi clients, and durable accounting across PostgreSQL or Linux
SQLite. It includes crash/replay, admission faults and offline paired SQLite
recovery. For full setup and scenario contracts, read the
[maintainer guide](../docs/development/system-e2e.md).

## Prepare the environment

Run from the repository root on Linux. Prepare the following before starting
the runner; it does not implicitly install dependencies or build the gateway:

- A gateway built with `sqlite-backend,embedded-console-ui` for the two-backend matrix.
- Built Console assets, installed Console dependencies and Playwright Chromium.
- Python 3.11+, Node, OpenSSL and a C compiler.
- Docker for the PostgreSQL backend only.
- The exact client versions in [`clients.json`](clients.json), without personal login state.
- The external Codex test plugin pinned by
  [`tests/fixtures/codex-plugin.json`](../tests/fixtures/codex-plugin.json).

```bash
python3 -m venv target/e2e-venv
target/e2e-venv/bin/pip install --require-hashes -r e2e/requirements.txt
target/e2e-venv/bin/python -m unittest discover -s e2e -p 'test_*.py'
node --test e2e/browser-checks.test.mjs
export AI_GATEWAY_TEST_CODEX_PLUGIN="$(scripts/prepare-connector-tests.sh)"
```

The external plugin needs an explicit read-only library path and companion
`codex-test.tar.gz` package. Missing or invalid fixtures fail preflight; they
do not silently skip acceptance scenarios. The runner uses a private plugin
directory and records the fixture digest. It never reads local gateway
configuration or real-upstream credentials.

## Run either backend

```bash
target/e2e-venv/bin/python e2e/run.py
target/e2e-venv/bin/python e2e/run.py --backend sqlite
```

Choose `--binary` for another built gateway, or `--codex` and `--pi` for explicit
client executables. Output and isolation requirements are described in the
[maintainer guide](../docs/development/system-e2e.md); do not point the suite at
an ordinary development or production database.

## Understand fixture builds

Trusted local SDK examples are the explicit exception to the no-implicit-build
rule. Preflight builds and packages them, then validates identity and digest
before installing through the real Console API:

| Fixture | Default build/package paths | Explicit reuse |
| --- | --- | --- |
| Response adapter | `target/response-adapter-tests-build`, `target/response-adapter-tests/<digest>` | `AI_GATEWAY_TEST_RESPONSE_PLUGIN` from `scripts/prepare-response-adapter-tests.sh` |
| Pure usage parser | `target/usage-parser-tests-build`, `target/usage-parser-tests/<digest>` | `AI_GATEWAY_TEST_USAGE_PLUGIN` from `scripts/prepare-usage-parser-tests.sh` |

Reuse requires an absolute read-only library path and its companion
`example-response-adapter-test.tar.gz` or `example-usage-parser-test.tar.gz`.
The usage fixture must advertise pure usage parsing, not response adaptation.

## Interpret coverage and results

| Area | What acceptance checks |
| --- | --- |
| Native lifecycle | Package upload, password reauthentication, activation, independent settings, stale-write rejection and settings persistence across disable/re-enable |
| Embedded plugin UI | Real upload and generated settings form, server ETag, persisted changes and enable/disable controls |
| OAuth planning | Provider authorization plan and host callback-state rejection without contacting the provider |
| Response adaptation | Same-format Chat/Responses JSON/SSE, request-local state, pinned settings, protected metering and terminal semantics, malformed/truncated responses |
| Usage selection | Unchanged response bytes, OpenAI/Anthropic dialect normalization and custom pure parsing, exact counters, logs, cost, balance and quota |

Response acceptance runs after durability and SQLite recovery, with its own
synthetic key and balance baseline. It accounts separately for thirteen
dispatched durable requests; the protocol-denied request uses another key and
must neither dispatch nor consume quota. Completed, incomplete and cancelled
Responses terminals retain their meaning without synthesized completion or
`[DONE]`. Incomplete/cancelled cases retain raw 5/2 usage and log
`upstream_sse_error`; failures follow the ordinary zero-cost policy.

The five usage scenarios follow adapter settlement with another baseline.
They verify canonical `input/cache-read/cache-write/output/reasoning` counters
`5/3/0/2/0` and `0.000006` per-request cost. The synthetic Anthropic usage
dialect is not an Anthropic transport implementation. Invalid custom counters
and unknown usage are native/offline negative tests, not successful unpriced
requests added to this suite. Reports separate `usage_settlement` and adapter
settlement and count each durable request once.

## Limits

Successful OAuth exchange, refresh/quota and Codex-channel forwarding remain
deterministic Rust integration coverage. Real CLI tool cycles here use ordinary
channels. Response-adapter coverage does not include WebSocket adaptation,
cross-format conversion or adapter-inclusive SQLite backups.

This suite complements, rather than replaces, fast
[`web/console/e2e/`](../web/console/e2e/) mocks, Rust integration tests,
[paid opt-in smoke](../docs/development/real-upstream-smoke.md) and
[opt-in performance runs](../docs/development/forwarding-performance.md).
