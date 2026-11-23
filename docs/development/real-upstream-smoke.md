# Real upstream smoke test

> Status: current. This is an explicit, paid validation path and is never part
> of ordinary tests.

Run this suite only with explicit authorization and dedicated, capped test
credentials. It makes real provider requests; the local-only CLI compatibility
test below does not. Start by configuring the ignored secrets file, then choose
the narrowest authorized mode.

## On this page

- [Configure credentials and run](#local-configuration)
- [Understand what the paid suite verifies](#forwarding-coverage)
- [Check Codex profile limitations](#codex-target-profile)
- [Run the separate local CLI fixture](#optional-local-codex-fallback-compatibility)
- [Review cost and evidence boundaries](#paid-smoke-boundaries)

## Forwarding coverage

`tests/real_upstream/` verifies the gateway against explicitly configured real,
OpenAI-compatible upstreams. It is deliberately ignored by normal `cargo test`
runs. Both profiles exercise Chat Completions (JSON and SSE), Responses SSE and Responses WebSocket. The default `openai_compatible` profile also exercises non-streaming HTTP Responses. `codex_oauth` selects a target gateway whose Responses channel uses the `codex` plugin; its supported transports are SSE and WebSocket. This profile name is a test setting, not a connector ID.

Responses WebSocket can optionally use a separate channel base URL and API key
while retaining `REAL_UPSTREAM_RESPONSES_MODEL`. A complete optional Images
configuration enables two additional paid tests for non-streaming generation
and multipart edit. A complete optional Search configuration enables one paid
standalone `/v1/alpha/search` test. Omitting an optional group skips its tests.

The test constructs an in-memory control-plane snapshot and drives the public
Axum router. It verifies the production forwarding path—model aliasing,
replacement of the synthetic client Bearer credential with the configured
upstream Bearer credential, `reqwest` forwarding, SSE streaming, downstream
and upstream WebSocket upgrades, usage extraction, price-snapshot binding, and
terminal request-log costs—without requiring PostgreSQL or modifying a shared
control plane. For SSE the smoke compares logged usage to the wire summary:
a Chat finish or usage-only chunk, or a Responses `response.completed` event.
An earlier delta with usage is not evidence of terminal usage.
The WebSocket case opens a client connection to the test's
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
| `REAL_UPSTREAM_RESPONSES_PROFILE` | `openai_compatible` (default) or `codex_oauth` for a Codex-backed target. See the [profile boundary](#codex-target-profile). |
| `REAL_UPSTREAM_WEBSOCKET_BASE_URL` | Optional Responses WebSocket channel base URL override. Must be paired with `REAL_UPSTREAM_WEBSOCKET_API_KEY`; otherwise WebSocket uses the default URL/key. |
| `REAL_UPSTREAM_WEBSOCKET_API_KEY` | Optional Responses WebSocket credential override. Must be paired with `REAL_UPSTREAM_WEBSOCKET_BASE_URL`. |
| `REAL_UPSTREAM_SEARCH_BASE_URL` | Optional standalone web-search channel base URL. Setting any Search value requires all three Search settings. The Gateway appends `/v1/alpha/search`. |
| `REAL_UPSTREAM_SEARCH_API_KEY` | Optional dedicated standalone web-search credential. |
| `REAL_UPSTREAM_SEARCH_MODEL` | Optional Search-capable Responses model. The target channel must declare standalone web-search support. |
| `REAL_UPSTREAM_IMAGES_BASE_URL` | Optional Images channel base URL. Setting any Images value requires all three Images settings and enables both paid Images tests. |
| `REAL_UPSTREAM_IMAGES_API_KEY` | Optional dedicated Images credential. |
| `REAL_UPSTREAM_IMAGES_MODEL` | Optional low-budget model supporting both `/v1/images/generations` and multipart `/v1/images/edits`. |
| `REAL_UPSTREAM_TIMEOUT_SECONDS` | Optional per-request limit; defaults to 60 and must be at least 3. |

After explicit authorization, run from the repository root:

```bash
./scripts/run-real-upstream-smoke.sh
```

The default `--all` mode runs the selected profile and adds Search and Images when configured. For an explicitly authorized,
Search-only diagnostic:

```bash
./scripts/run-real-upstream-smoke.sh --search-only
```

For an explicitly authorized, lower-cost
diagnostic rerun of only the paid Images edit case:

```bash
./scripts/run-real-upstream-smoke.sh --images-edit-only
```

To use a differently named local secrets file, keep it ignored and specify:

```bash
REAL_UPSTREAM_ENV_FILE=/secure/path/upstream-smoke.env \
  ./scripts/run-real-upstream-smoke.sh
```

The script sources only this developer-controlled shell assignment file, never
prints its values, disables shell tracing, validates the required settings and
optional setting groups, and then runs the ignored tests with
`RUN_REAL_UPSTREAM_SMOKE=1`. With no Images settings it passes test-harness
skip filters for optional modules; partially configured WebSocket, Search, or
Images overrides or an unknown Responses profile fail before Cargo starts.

## Codex target profile

With `REAL_UPSTREAM_RESPONSES_PROFILE=codex_oauth`, the SSE case sends ordinary client JSON with `Content-Encoding: zstd`; the outer gateway decodes it before applying request policy. SSE and WebSocket clients send `max_output_tokens`, which the target Codex connector removes before dispatch. Both cases must complete and retain terminal usage.

The script omits the unsupported non-streaming Responses case for this external profile. A passing Codex smoke therefore does **not** verify unary Responses or its rejection. Local integration tests load the actual [pinned Codex library](../../tests/fixtures/codex-plugin.json) and verify that a unary request with no eligible channel returns `503 no_healthy_channel` without upstream dispatch or charge. See [protocol selection](usage-normalization.md).

## Safety and scope

### Optional local Codex fallback compatibility

This separate, manually selected integration test uses an installed Codex CLI
(`codex`, or `CODEX_BIN`) against loopback Gateway/upstream fixtures only:

```bash
cargo test --locked --test websocket_integration \
  codex_cli_falls_back_to_http_for_unavailable_websocket_routes -- --ignored --exact
```

It creates disposable `HOME`/`CODEX_HOME` directories, clears inherited
environment settings, supplies a synthetic API key, and returns fixed mock
responses without tool calls. It does not use real upstream credentials, contact a paid
provider, or replace the mandatory real-upstream smoke for forwarding changes.
The child has a bounded lifetime and is killed/reaped on timeout.

The local cases exercise both authorization scopes: an HTTP-only scope gets
HTTP 426 during upgrade; a mixed-capability scope upgrades, then rejects the
HTTP-only model with a wrapped 426 error. Both must attempt WS, complete via
exactly one mock HTTP request, and preserve matching request-log statuses.
Ordinary Rust CI ignores this test because it does not install Codex;
deterministic Gateway protocol/authorization/health tests remain part of
`websocket_integration`.

The idle-expiry case also runs against loopback fixtures. It checks that the CLI receives
`previous_response_not_found` and sends a full request containing the original user message,
tool call and tool result; the expired delta must never reach the mock upstream.
Use an installed Codex CLI or set `CODEX_BIN`:

```bash
cargo test --locked --test websocket_integration \
  ws_state_recovery::codex_cli_resends_full_tool_history_after_idle_expiry -- --ignored --exact
```

### Paid smoke boundaries

- Use a dedicated credential with a strict spending cap, model allowlist, and
  rate limit. Never use a production credential.
- Tests run serially. The default profile asks for one output token in the four HTTP/SSE requests; provider-specific output minimums can still incur a charge. In the Codex profile the Responses SSE case asks for an exact `OK` response; the target strips the client output-token cap before dispatch.
  The Chat Completions streaming request includes
  `stream_options.include_usage=true` so its final SSE usage can be verified.
  The Responses WebSocket request sends one
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
  Images usage, a nonempty `data` array whose entries contain a nontrivial
  `b64_json` payload or HTTP(S) URL, price-snapshot binding, and a successful
  terminal `images_generation` or `images_edit` request log. The script prints
  only sanitized elapsed milliseconds, output count, and token counts for
  these two tests; it never prints image data or credentials. Provider minimum
  pricing and output behavior still apply, so use a credential with an
  Images-specific spending cap.
- When Search settings are present, one non-streaming request asks for a single
  authoritative Rust source using structured commands without synthetic `input`,
  without inventing conversation history. It asserts a nonempty `output`, preserves opaque
  optional `results`, verifies the distinct `web_search` log
  operation, and prints only elapsed time, output character count, and result
  count. Use a credential with a Search-specific spending cap.
- Do not run it in ordinary PR checks; the suite remains opt-in and paid.
- Do not add real credentials to `./config/*`, test source, shell history, or
  logs.
- The existing local and PostgreSQL integration tests remain the coverage for
  control-plane persistence, management APIs, failure injection, and
  deterministic edge cases, including Images generation/edit routing, multipart
  spool/replay, Codex OAuth data-URL adaptation and image-turn headers,
  pre-upstream streaming rejection, and the no-automatic-retry boundary.
