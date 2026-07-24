# Real upstream smoke test

> Status: current. This is an explicit, paid validation path and is never part
> of ordinary tests.

`tests/real_upstream/` verifies the gateway against one real,
OpenAI-compatible upstream. It is deliberately ignored by normal `cargo test`
runs: it has separate Chat Completions and Responses tests for non-streaming
and streaming requests.

The test constructs an in-memory control-plane snapshot and drives the public
Axum router. It verifies the production forwarding path—model aliasing,
replacement of the synthetic client Bearer credential with the configured
upstream Bearer credential, `reqwest` forwarding, SSE streaming, usage
extraction, price-snapshot binding, and terminal request-log costs—without
requiring PostgreSQL or modifying a shared control plane.

## Local configuration

The only supported `.env` use in this repository is this test script. Copy the
tracked template and keep the resulting file local:

```bash
cp .env.real-upstream.example .env.real-upstream
```

Fill these values in `.env.real-upstream`:

| Variable | Meaning |
| --- | --- |
| `REAL_UPSTREAM_BASE_URL` | Channel base URL. The gateway appends the selected `/v1/...` route. It may include a provider path prefix, but not the final route, query, fragment, or credentials. |
| `REAL_UPSTREAM_API_KEY` | Dedicated upstream test credential. |
| `REAL_UPSTREAM_CHAT_COMPLETIONS_MODEL` | A low-cost model supported by `/v1/chat/completions`. |
| `REAL_UPSTREAM_RESPONSES_MODEL` | A low-cost model supported by `/v1/responses`. |
| `REAL_UPSTREAM_TIMEOUT_SECONDS` | Optional per-request limit; defaults to 60 and must be at least 3. |

Run:

```bash
./scripts/run-real-upstream-smoke.sh
```

To use a differently named local secrets file, keep it ignored and specify:

```bash
REAL_UPSTREAM_ENV_FILE=/secure/path/upstream-smoke.env \
  ./scripts/run-real-upstream-smoke.sh
```

The script sources only this developer-controlled shell assignment file, never
prints its values, disables shell tracing, validates required settings, and
then runs the ignored test with `RUN_REAL_UPSTREAM_SMOKE=1`.

## Safety and scope

- Use a dedicated credential with a strict spending cap, model allowlist, and
  rate limit. Never use a production credential.
- The four requests run serially. Each sends a deliberately short prompt and
  asks for one output token; provider-specific minimum-output behavior may
  still incur a small charge. The Chat Completions streaming request includes
  `stream_options.include_usage=true` so its final SSE usage can be verified.
- Do not run it in ordinary PR checks. CI automation is intentionally not part
  of this change.
- Do not add real credentials to `./config/*`, test source, shell history, or
  logs.
- The existing local and PostgreSQL integration tests remain the coverage for
  control-plane persistence, management APIs, failure injection, and
  deterministic edge cases.
