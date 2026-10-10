# Connector response adapters

> 状态：当前。Protocol-3 SDK, capability preflight, host JSON/SSE execution and
> runnable example are implemented. Same-format adaptation has deterministic
> and PostgreSQL/SQLite system E2E coverage.

## Scope and versioning

The [SDK command contract](../../crates/connector-sdk/docs/commands.md) is the
authoritative metadata and command reference. Native ABI remains 1 and the SDK
package remains `0.2.0`; protocol negotiation is independent. Existing
protocol-1/2 libraries keep their pass-through behavior. Codex protocol 3 keeps
all response modes `passthrough`; its independent usage profile reuses the
general Responses parser.
Protocol 3 adds `attempt.describe/v1`, required capability booleans and explicit
per-operation transport/response entries.

Only HTTP Chat Completions and HTTP Responses can opt into `json` or `sse`
adaptation. This is same-format response normalization, not a cross-protocol
bridge. WebSocket, standalone search and Images may declare `passthrough` only.
No implicit protocol grants, client `ApiFormat` changes, or JSON/SSE framing
changes are permitted.

## Host responsibilities and ordering

The configured plugin generation is pinned for the complete request, including
its library, settings, descriptor and all response calls. Descriptors can be
cached per operation/generation. The compiled runtime intersects persisted
transport permissions with supported plugin protocols; persistent permissions
are not automatically extended.

Request policy, model selection, transforms, plugin request preparation,
outbound guards and authentication keep their existing order. The host owns
the selected route, final status and headers, transport dispatch, framing,
admission, billing and durable logging. A response command cannot authorize
retries after dispatch or perform plugin-owned HTTP requests.

After receiving upstream headers, explicit `json` mode buffers at most 8 MiB
and validates adapted JSON before downstream dispatch. An oversized or invalid
body fails closed. Without opt-in, responses are not newly buffered.

Explicit `sse` mode parses bounded SSE frames incrementally, dispatches one
data payload at a time, and encodes the returned events using host framing.
The existing 8 MiB frame ceiling also bounds envelope bytes.
It never interprets arbitrary TCP/HTTP chunks as logical events. Input event
data is bounded to 1 MiB, state to a 64 KiB JSON object, each result to 16 events,
and serialized output metadata to the ABI's 1 MiB ceiling. Comment-only and
data-less frames are not arbitrary adaptation callbacks.

State starts at `{}` and is owned only by the current request. The host passes
the returned state to the next synchronous call; plugins must not retain
payloads or create mutable request globals. Finish uses an empty data body,
runs before yielding the actual terminal and must return no events; it cannot
synthesize or repeat successful completion. EOF without a terminal may call
finish before failing. Missing Chat `[DONE]` or a missing Responses terminal
causes stream failure rather than EOF success.

## Metering, terminal and failure boundaries

The host independently inspects original and adapted protected semantics.
Recursive `usage`, `id`, `model`, `type`, `status`, `error`, `finish_reason`,
`index`, `call_id`, `name`, `namespace`, and `role` fields must be preserved;
response text adaptation cannot turn a failure into success,
remove/add/change known usage, or manufacture completion. Upstream usage is
the sole financial source. Response-adapter output is never authoritative
metering. The independently selected [usage parser](usage-normalization.md)
interprets original upstream counts, not the adapted presentation.
The complete accepted protected projection and tests live in host code, not in
the SDK's generic size validators.

Inputs with usage, terminal, sequence-number or tool-call semantics produce
exactly one corresponding output event. Only content-only events without these
constraints can fan out within the bounded output contract. A descriptor cannot
select JSON adaptation while declaring `successful_response_is_sse:true`.

Malformed JSON, invalid metadata/state, exceeded limits, plugin errors and
protected-semantic mismatches fail closed. JSON adaptation errors produce
sanitized HTTP failures. Streaming errors abort the response body; if headers
have not flushed, the client may observe a closed connection instead of an
HTTP status. There is no retry, candidate switching or fabricated successful
terminal. Incomplete streams are not repaired by `finish`.

Synchronous native plugins must be pure and promptly bounded, but these are
contract requirements rather than sandbox guarantees. The
[native plugin trust boundary](connector-plugins.md) still applies: reviewed
administrator-installed code executes with host privileges.

## Runnable fixture and verification

Build the example from the repository root:

```sh
cargo build --locked -p ai-gateway-connector-sdk --example response_adapter
cargo test --locked -p ai-gateway-connector-sdk --all-targets
```

The artifact is `target/debug/examples/libresponse_adapter.so` on Linux, ID
`example-response-adapter`, protocol 3. Settings schema 1 contains `label`,
`mode` and `supported_protocols`; defaults are documented in the
[SDK README](../../crates/connector-sdk/README.md).

Normal JSON mode prefixes message/output text with `label`. Normal SSE mode
prefixes each text-containing event with `label + text_events + ":"`, starting
the request-local `text_events` counter at 1. It keeps numeric state only and
modifies text deltas, passing
usage, terminal events and `[DONE]` unchanged. Finish emits no events. The
`invalid` and `metering_tamper` modes intentionally violate the contract for
host failure tests; they are not production host settings or supported
adaptation behavior.

SDK tests cover type round trips, descriptor protocol/mode validation,
state/event/count/aggregate limits, fixture settings, content-only adaptation,
terminal preservation and independent request state. Host tests also cover
transport intersection, zero overlap, protected semantics and error-then-EOF
stream lifecycle.

The implementation passed the full Rust workspace suite with
`embedded-console-ui,sqlite-backend` against isolated PostgreSQL, plus both
[system E2E backends](system-e2e.md). Those system tests cover Chat/Responses
JSON/SSE, generation pinning, interleaved state, genuine incomplete/cancelled
terminals, fail-closed output and independent durable settlement. Failed
requests retain observed usage but preserve the existing zero-charge policy.

The authorized [real-upstream smoke](real-upstream-smoke.md) passed existing
passthrough compatibility paths, including configured optional Images/search
checks. Native response adaptation itself uses the deterministic local fixture;
the smoke does not claim coverage of a production response-adapter plugin.
WebSocket adaptation, cross-format conversion and adapter-inclusive backup
recovery are outside this acceptance scope.

## Related documents

- [Connector architecture](connector-plugins.md)
- [System E2E](system-e2e.md)
- [Real-upstream verification](real-upstream-smoke.md)
