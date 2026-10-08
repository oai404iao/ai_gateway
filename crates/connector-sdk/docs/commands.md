# Gateway connector commands

> Status: Current SDK 0.2 command contract, native ABI v1.

The [native ABI](../README.md) describes loading and memory ownership. This
document describes the **additional gateway adapter contract**. A library can
pass ABI loading and still be unusable for routing if its commands or operations
do not satisfy this contract.

Manifest `protocol_version` negotiates metadata semantics independently of the
C ABI. Omission means legacy protocol 1. Protocol 2 provides configured
invocations and plugin-owned Codex request identity/privacy; Codex and plugins
declaring settings commands must declare 2. Generic stateless protocol-1
plugins remain supported. Older gateways reject the new manifest field rather
than silently interpreting incompatible metadata. Settings commands additionally
carry their own `/v1` suffix. Unknown protocol versions fail before dispatch.

## Names and common rules

Manifest `operations` uses these exact identifiers:

| Operation | Protocol values |
| --- | --- |
| `chat_completion` | `non_stream`, `sse` |
| `responses` | `non_stream`, `sse` |
| `responses-ws` | `websocket` |
| `web_search` | `non_stream` |
| `images_generation` | `non_stream` |
| `images_edit` | `non_stream` |

`websocket` is a protocol, not an alias for operation `responses-ws`.
Declare only implemented operations. An installed manifest alone does not
create an access, capability, route, or authorization grant.

Every routable external connector declares and implements:

```text
attempt.capabilities
attempt.body
attempt.target
attempt.headers
```

Images edit has the additional contract below. Calls are synchronous and may
occur concurrently; do not rely on a particular previous call, mutable
per-request global state, or retaining input pointers. Network dispatch,
timeouts, streaming, credentials, admission, billing, and durable state remain
host-owned.

All input/output metadata is a JSON object. Unless stated otherwise, input
and output raw bodies are empty. Do not wrap common command outputs in
`{"result":...}`: that envelope belongs only to the Codex control adapter.
Unknown metadata fields may contain host extensions; do not echo secrets back
unnecessarily.

## Declarative settings

A configurable plugin declares all three commands `settings.describe/v1`,
`settings.validate/v1`, and `settings.compile/v1`. A plugin without settings
declares none. Partial declarations are invalid; `settings.migrate/v1` is
optional only for a configurable plugin.

`settings.describe/v1` receives `{}` and returns a descriptor directly:

```json
{
  "schema_version": 1,
  "title": {"en": "Example settings"},
  "fields": [
    {
      "key": "mode",
      "label": {"en": "Mode"},
      "required": true,
      "type": "enum",
      "options": [{"value": "safe", "label": {"en": "Safe"}}]
    }
  ],
  "defaults": {"mode": "safe"}
}
```

Field types are `string` (required `max_length`), `boolean`, `integer`
(required `minimum` and `maximum`), and `enum` (required `options`).
Each option has a string `value` and localized `label`. Fields may include a
localized `description`. Localization is a bounded language-tag-to-plain-text
map, not HTML. Unknown attributes and values are rejected. Nested values,
secrets, executable expressions, and plugin JavaScript are not supported.

The SDK bounds descriptions/documents to 64 KiB, 64 fields, 4096 characters per
text, eight locales, and JavaScript-safe integer ranges. Defaults must satisfy
the descriptor. The host materializes defaults when explicitly initializing
settings; changing plugin defaults must not silently change existing settings.

`settings.validate/v1` receives `{"schema_version":1,"values":{"mode":"safe"}}`
and returns `{"valid":true,"errors":[]}` or a nonempty error list with
`{"field":"mode","code":"invalid_value"}` entries. Fields must exist in the
descriptor; codes use bounded lowercase ASCII letters, digits, and underscores.
Validation is deterministic and does not mutate input or perform I/O.

`settings.compile/v1` receives the same document after successful validation
and returns `{"config":{...}}`. The config is opaque to the host and consumed
only by the plugin. Every non-settings command on the configured instance
receives this immutable object in reserved metadata member `settings`.
Callers cannot supply or override that member. Input/output bodies of settings
commands are empty.

`settings.migrate/v1` receives
`{"from_schema_version":0,"values":{...}}` and returns
`{"schema_version":1,"values":{...}}`. The host revalidates the result before
saving. Migration must reject unsupported upgrades/downgrades rather than
silently discarding unknown data. It does not update database state itself.

An invocation instance binds a library artifact digest and settings revision.
Each logical request or maintenance operation pins one instance throughout its
execution. New publications cannot change an in-flight instance, and native
libraries remain resident for the process lifetime even after deactivation.

## `attempt.capabilities`

Input metadata:

```json
{"operation":"responses"}
```

Output metadata:

```json
{
  "preserves_affinity_on_failure": false,
  "successful_response_is_sse": false,
  "changes_request_body": false
}
```

All three values are booleans. `successful_response_is_sse` means a successful
response is intrinsically SSE even when its upstream Content-Type is missing
or misleading; it is **not** merely a declaration that the operation supports
client-selected SSE. Ordinary Responses pass-through plugins should return
`false`.

The generic host adapter consumes the SSE flag, always treats plugin body
adaptation as potentially modifying bytes, does not preserve failed-request
affinity, and never automatically retries a dispatched plugin request. The
Codex adapter additionally uses the other two flags. A plugin cannot use these
fields to acquire retry privileges or bypass admission.

## `attempt.body`

Input metadata:

```json
{"operation":"responses","protocol":"sse"}
```

The raw input body is the host-prepared JSON request after client policy,
model selection, and transforms. Return the adapted raw JSON bytes and an
object metadata value (normally `{}`). A pass-through plugin returns the
original bytes without reserialization.

The host rejects non-object JSON, changes to the selected top-level `model`,
and any addition/removal/change of top-level `service_tier` (including adding
`null` where the field was absent). These checks keep routing, Fast filtering,
and billing bound to host decisions. Multipart Images edit uses the separate
planning commands instead of sending image files to this command.

## `attempt.target`

Generic input metadata:

```json
{
  "operation":"responses",
  "base_url":"https://upstream.example/base",
  "path":"/v1/responses",
  "query":"trace=1"
}
```

`query` is a string without the leading `?`, or JSON `null`. `path` is supplied
for generic connectors; the Codex adapter omits it and derives the endpoint
from `operation`. Return:

```json
{"url":"https://upstream.example/base/v1/responses?trace=1"}
```

The host requires the configured base origin and user-info, forbids fragments,
and confines the result to the base path or one of its descendants. It rejects
cross-origin URLs and path escapes. Do not use URL-join behavior that silently
drops a configured path prefix. For WebSocket, follow the host's HTTP(S)
upstream target convention; the gateway owns dialing and protocol conversion.

## `attempt.headers`

Generic input metadata:

```json
{
  "operation":"responses",
  "protocol":"sse",
  "headers":{"content-type":"application/json","accept":"text/event-stream"}
}
```

Headers are represented as a string-to-string object, not a repeated-header
list. Return both `set` and `remove`, even when empty:

```json
{"set":{"x-provider-feature":"enabled"},"remove":["x-unused"]}
```

The host validates header names/values, rejects forbidden generated headers
(including transport/hop-by-hop, blocked forwarding metadata, cookies, and
WebSocket handshake headers), and applies the plan atomically. Generic
connector credential injection runs **after** this plan; do not manufacture
gateway-managed bearer/custom-header secrets. Plugin-provided `Authorization`
is removed before the selected host authentication is applied: no-auth leaves
it absent, and custom-header authentication sets only its configured header.
Shared outbound policy remains
in force immediately before transport dispatch.

The Codex adapter first invokes `attempt.context` with `operation`,
`credential_id`, a per-logical-request `request_id`, nullable `affinity_hash`
(32 integer bytes), and client `headers`. The returned object is opaque to the
host and supplied as `request_context` on body/header calls. The plugin owns
installation-ID derivation, request identity, and privacy normalization.

Codex body/header calls also receive the selected credential's `access_token`,
nullable `account_id`, and `is_fedramp`; header calls include the current
`headers`. These credentials are host-selected, not client input. Its plan must retain
the expected bearer authorization. Configured provider identity and privacy
normalization belong to the plugin; the host does not interpret provider
settings or map them into a host privacy schema.

## Images edit planning

Plugins advertising `images_edit` must implement
`attempt.image_edit_plan` and `attempt.image_part_plan` in addition to the
four common commands. The host owns multipart capture, replay, memory limits,
temporary files, and streamed base64; raw images are never copied into ABI
metadata.

`attempt.image_edit_plan` receives metadata
`{"image_count":1,"mask_count":0}` and a raw UTF-8 JSON array of multipart text
fields, preserving their order:

```json
[{"name":"model","value":"selected"},{"name":"prompt","value":"Edit this"}]
```

The result must select one explicit `body_mode`:

| Output metadata | Raw output body | Host behavior |
| --- | --- | --- |
| `{"body_mode":"replay_multipart"}` | Empty | Replay the captured multipart body, or rebuild only the selected model/ignored fields; preserve image files, masks, and boundary |
| `{"body_mode":"json_base64","prefix_bytes":123}` | Concatenated JSON prefix and suffix | Split at byte offset `prefix_bytes` and stream image fragments between those ranges |

Missing/unknown modes fail closed. `json_base64` is rejected whenever
`mask_count` is nonzero; ABI v1 provides no streamed mask projection. Use
`replay_multipart` for a provider requiring ordinary multipart/mask semantics.
The host enforces the resulting Content-Type after plugin header adaptation:
the exact captured multipart boundary or `application/json`, respectively.
Plugins cannot independently select a conflicting body encoding/header.

In `json_base64` mode, for each image, `attempt.image_part_plan` receives
`{"content_type":"image/png","file_name":"input.png","index":0}`; either of the
first two fields may be null, and `index` is zero-based. It returns metadata
`{"prefix":"...","suffix":"..."}`. These UTF-8 strings surround the base64
image bytes that the host streams directly from its spool. They must account
for JSON quoting and commas between images. Input/output raw bodies for the
part command are empty.

`replay_multipart` does not invoke the part planner, but a routable Images edit
manifest must still declare it. The current Codex provider selects
`json_base64`; this does not require generic providers to use that mode.

For `json_base64`, the host validates the assembled JSON structure using a bounded skeleton
without retaining encoded image contents; selected `model` must be preserved
and `service_tier` must not appear. Do not use these commands to log image
contents, filenames, multipart fields, or credentials. This contract does not
provide arbitrary plugin-authored multipart transformations.

## Codex control adapter

These commands are specific to connector ID `codex`; generic connectors do
not gain OAuth lifecycle behavior by declaring them. The current Codex host
adapter requires the following exact command set in addition to `attempt.*`:

| Commands | Purpose / input metadata |
| --- | --- |
| `endpoints` | `{}` → issuer, Responses base URL, and redirect URI |
| `authorize_url` | `endpoints`, PKCE `challenge`, OAuth `state`, configured `identity` |
| `parse_callback` | callback `url` |
| `parse_identity`, `parse_expiration` | `token` |
| `exchange_plan` | `endpoints`, authorization `code`, PKCE `verifier` |
| `refresh_plan` | `endpoints`, `refresh_token` |
| `models_plan`, `quota_plan` | `endpoints`, `identity`, `access_token`, nullable `account_id`, `is_fedramp` |
| `quota_reset_plan` | Same credential context plus `redeem_request_id` |
| `exchange_parse`, `refresh_parse`, `models_parse`, `quota_parse`, `quota_reset_parse` | HTTP `status`; raw body is the provider response |

Successful metadata uses `{"result": VALUE}`. Plan results are objects with
`method`, `url`, and string-to-string `headers`; their separate raw body holds
the outbound HTTP payload. The host validates URL/auth/identity constraints
and performs HTTP itself. Parse results must deserialize into the host's
corresponding token, identity, model, quota, or reset types.

Provider protocol failures use `{"protocol_error": VALUE}` with an empty body
and ABI success status, where `VALUE` is the shared Codex error representation.
This is distinct from ABI `PluginCallError`. The concrete result/error schemas
are coupled to the host Codex adapter and external connector release; neither
the generic loader nor ABI version alone establishes their compatibility.

## Errors and memory

For common-command failures return `PluginCallError` (ABI status 1), with a
1–64 character lowercase ASCII letter/digit/underscore code and an empty raw
body. The host discards the message. Generic adapters report sanitized connector
failures; Codex additionally recognizes specific operation-validation codes.
Do not encode arbitrary errors as successful metadata.

Respect the [ABI allocation, limits, panic, and sensitive-lifetime
rules](../README.md). In particular, input bytes are borrowed only during the
call, output allocations are separately owned and writable, and the host wipes
them before plugin release. Semantic `PluginOutput` fields transferred to a
caller are not automatically secret-erased; use `zeroize_json` and existing
secret wrappers when their owned contents are no longer needed.

## Implementation references

- [Generic host adapter](../../../src/application/connector.rs)
- [Codex data-plane adapter](../../../src/application/codex/attempt.rs)
- [Codex control adapter](../../../src/application/codex/protocol.rs)
- [Multipart host adapter](../../../src/application/request_body.rs)
- [Runnable Responses example](../examples/responses.rs)
