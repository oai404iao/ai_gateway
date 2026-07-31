# Real upstream smoke test

> Status: current. This is an explicit, paid validation path and is never part
> of ordinary tests.

`tests/real_upstream/` verifies the gateway against explicitly configured real,
OpenAI-compatible upstreams. It is deliberately ignored by normal `cargo test`
runs. The required baseline has separate Chat Completions and Responses tests
for non-streaming and streaming requests, plus a Responses WebSocket request.
Responses HTTP/SSE and WebSocket `input` values use explicit message arrays.

Responses WebSocket can optionally use a separate channel base URL and API key
while retaining `REAL_UPSTREAM_RESPONSES_MODEL`. A complete optional Images
configuration enables two additional paid tests for non-streaming generation
and multipart edit. Omitting all Images settings skips those two tests.

The test constructs an in-memory control-plane snapshot and drives the public
Axum router. It verifies the production forwarding path—model aliasing,
replacement of the synthetic client Bearer credential with the configured
upstream Bearer credential, `reqwest` forwarding, SSE streaming, downstream
and upstream WebSocket upgrades, usage extraction, price-snapshot binding, and
terminal request-log costs—without requiring PostgreSQL or modifying a shared
control plane. The WebSocket case opens a client connection to the test's
random local Gateway listener, sends a deterministic `response.create` frame,
and consumes events through `response.completed`.

## Local configuration

The only supported `.env` use in this repository is this test script. Copy the
tracked template and keep the resulting file local:

```bash
cp .env.real-upstream.example .env.real-upstream
```

Fill these values in `.env.real-upstream`:

| Variable | Meaning |
| --- | --- |
| `REAL_UPSTREAM_BASE_URL` | Required default channel base URL for Chat Completions and Responses HTTP/SSE. The gateway appends the selected `/v1/...` route. It may include a provider path prefix, but not the final route, query, fragment, or credentials. |
| `REAL_UPSTREAM_API_KEY` | Required default credential for Chat Completions and Responses HTTP/SSE. |
| `REAL_UPSTREAM_CHAT_COMPLETIONS_MODEL` | A low-cost model supported by `/v1/chat/completions`. |
| `REAL_UPSTREAM_RESPONSES_MODEL` | A low-cost model supported by Responses HTTP/SSE and WebSocket. |
| `REAL_UPSTREAM_WEBSOCKET_BASE_URL` | Optional Responses WebSocket channel base URL override. Must be paired with `REAL_UPSTREAM_WEBSOCKET_API_KEY`; otherwise WebSocket uses the default URL/key. |
| `REAL_UPSTREAM_WEBSOCKET_API_KEY` | Optional Responses WebSocket credential override. Must be paired with `REAL_UPSTREAM_WEBSOCKET_BASE_URL`. |
| `REAL_UPSTREAM_IMAGES_BASE_URL` | Optional Images channel base URL. Setting any Images value requires all three Images settings and enables both paid Images tests. |
| `REAL_UPSTREAM_IMAGES_API_KEY` | Optional dedicated Images credential. |
| `REAL_UPSTREAM_IMAGES_MODEL` | Optional low-budget model supporting both `/v1/images/generations` and multipart `/v1/images/edits`. |
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
prints its values, disables shell tracing, validates the required settings and
optional setting groups, and then runs the ignored tests with
`RUN_REAL_UPSTREAM_SMOKE=1`. With no Images settings it passes a test-harness
skip filter for the Images module; partially configured WebSocket or Images
overrides fail before Cargo starts.

## Safety and scope

- Use a dedicated credential with a strict spending cap, model allowlist, and
  rate limit. Never use a production credential.
- The five baseline test cases run serially. The four Chat
  Completions/Responses HTTP/SSE requests ask for one output token;
  provider-specific
  minimum-output behavior may still incur a small charge. The Chat Completions
  streaming request includes `stream_options.include_usage=true` so its final
  SSE usage can be verified. The Responses WebSocket request sends one
  deliberately small, reviewable `response.create` frame and waits for
  `response.completed`. It validates the terminal event usage and the
  corresponding successful request log. If the Gateway reports the known
  transient `502 upstream_websocket_closed` terminal event, the smoke client
  may open a fresh WebSocket and resend the side-effect-free prompt, for at
  most three total attempts. Every failed attempt must retain a matching
  terminal log; the Gateway itself still performs no post-send retry and the
  smoke never falls back to HTTP. Deterministic integration tests separately
  assert that the production upstream connection negotiates
  `permessage-deflate`.
- When Images settings are present, generation requests one `1024x1024`,
  low-quality image and edit uploads one small compressed `1024x1024` PNG while
  requesting one low-quality `1024x1024` result. Both requests assert top-level
  Images usage, price-snapshot binding, and a successful terminal
  `images_generation` or `images_edit` request log. Provider minimum pricing
  and output behavior still apply, so use a credential with an Images-specific
  spending cap.
- Do not run it in ordinary PR checks. CI automation is intentionally not part
  of this change.
- Do not add real credentials to `./config/*`, test source, shell history, or
  logs.
- The existing local and PostgreSQL integration tests remain the coverage for
  control-plane persistence, management APIs, failure injection, and
  deterministic edge cases, including Images generation/edit routing, multipart
  spool/replay, Codex OAuth data-URL adaptation and image-turn headers,
  pre-upstream streaming rejection, and the no-automatic-retry boundary.
