# Connector SDK

> Status: Current native ABI v1 contract.

This development-only SDK defines the boundary between ai-gateway and
administrator-installed native connector libraries. The built-in `general`
connector needs no library. The loader validates the native ABI; the gateway
adapter additionally requires the [common command contract](docs/commands.md).
Implementing an arbitrary command is not enough to create a routable connector.
Provider-specific control commands must match their corresponding host adapter.

## Rust implementation

Build a separate crate with `[lib] crate-type = ["cdylib"]` and dependencies on
`ai-gateway-connector-sdk` and `serde_json`. The SDK requires neither an HTTP
client nor an asynchronous runtime.

The [Responses example](examples/responses.rs) is a complete generic connector:
it declares and implements all four required `attempt.*` commands, preserves
raw JSON request bytes, and leaves credential injection to the gateway.

```sh
# From the gateway repository root:
cargo build --locked -p ai-gateway-connector-sdk --example responses
cargo test --locked -p ai-gateway-connector-sdk --example responses
```

On Linux the built example is `target/debug/examples/libresponses.so`.
Install a protected mode-`0444` copy and configure `[[plugins]]` with
`id = "example-responses"` and its SHA-256 as described below. Then create a
matching upstream access/capability with operation `responses` and normal
gateway-managed credentials. The example appends `/v1/responses` to the
configured base URL (including any base path); for example,
`https://upstream.example` becomes `https://upstream.example/v1/responses`.
It supports `non_stream` and `sse`, not WebSocket or Images.

The host may call dispatch concurrently. Implementations must be thread-safe,
must not retain borrowed host memory, and must finish synchronous calls promptly.
Dispatch receives a JSON **object** for small control fields and a separate raw
body. Binary bodies need not be encoded as JSON. Successful output metadata is
also a JSON object. No Rust object, allocator, trait object, or `Future` crosses
the ABI.

## C ABI and allocation

[`include/ai_gateway_connector.h`](include/ai_gateway_connector.h) defines the
single exported symbol, `ai_gateway_connector_entry_v1`. The entry function
returns an immutable process-lived descriptor containing ABI version, exact
structure size, manifest JSON bytes, dispatch, and buffer release pointers.
The descriptor and functions must remain valid for the process lifetime.
The host and library must target the same platform architecture and C calling
convention; the ABI is not a wire protocol.

Lengths are unsigned 64-bit integers. Inputs are borrowed until dispatch
returns. A zero-length input may have a null pointer. Dispatch initializes
both output buffers on **every** return path. Empty output buffers must use
`{NULL, 0}`; nonempty buffers must be distinct allocations, readable for their
declared lengths, and released only by the originating library's `free_buffer`.
The host copies outputs before calling that release function exactly once
per output buffer, including empty buffers. Output allocations must be writable;
the host zeroizes their bytes before release, so `free_buffer` must not depend
on their contents. The Rust SDK also zeroizes allocations in its release
function. No function may unwind into C.

Statuses:

| Integer | Meaning | Output |
| --- | --- | --- |
| 0 | Success | JSON object metadata and raw body |
| 1 | Plugin error | `{"code":"lowercase_code","message":"..."}` and empty body |
| 2 | Panic contained by SDK | Empty buffers |
| 3 | Invalid input/output or size exceeded | Empty buffers |

Error codes contain 1–64 lowercase ASCII letters, digits, or underscores.
The host discards error messages and exposes only the code for typed mapping;
it must not log metadata, bodies, credentials, or provider error messages.
Rust export helpers catch unwinding panics from manifest/dispatch. An
abort-on-panic build, allocator failure, segmentation fault, or hang cannot be
contained.

The SDK installs one delegating panic hook and suppresses panic payload output
only on threads currently inside an ABI call. Other threads and non-ABI panics
retain the previous hook. Plugin implementations must not replace that hook:
doing so can expose credentials embedded in panic payloads.

## Sensitive data lifetime

The host zeroizes its serialized metadata scratch bytes, and validated plugin
output byte allocations are wiped before release. The SDK wipes owned output
JSON strings/keys and temporary output bodies after serialization or rejection.
`PluginOutput` debug formatting is redacted.

`zeroize_json(&mut Value)` is available to hosts/providers for owned JSON once
parsing is complete. Callers own successful `PluginOutput` fields and must
retain credentials only inside their existing secret-storage boundaries; the
struct does not erase fields automatically because ownership can transfer to
HTTP requests or typed domain values. Dispatch takes ownership of parsed input
metadata, so providers must erase secret-bearing strings/keys when no longer
needed. Borrowed host inputs are never modified.

These measures are **not a full-memory zeroization guarantee**: serde parsing,
serialization growth, JSON clones, HTTP buffers, panic payloads, and transferred
typed values may create other allocations outside this boundary. An abort,
malformed foreign pointer, or native plugin misbehavior can bypass cleanup.

Limits are 64 KiB for manifests, 1 MiB for metadata, 512 MiB for bodies,
128 bytes for a command, and 256 MiB for a library image. Manifest fields
`id`, `version`, `operations`, and `commands` are required; unknown fields fail
closed. IDs match `[a-z][a-z0-9_-]{0,63}`; `general` is reserved. Version is
nonempty printable ASCII (at most 128 bytes). Operations and commands must be
nonempty unique strings using ASCII letters, digits, `_`, `-`, `.`, `/`;
there may be at most 64 operations and 256 commands.

## Installation and trust

Linux is the initial supported host. Configure an absolute normalized path,
expected ID, and a 64-digit SHA-256 digest. Libraries must be regular files
owned by root or the gateway effective user with **no write permission bits**
(for example mode `0444`). Ancestor directories must be root/gateway-owned,
not group/other writable, and not symlinks. A symlink library is rejected.
Build output must be installed/copied into an appropriately protected path;
do not configure writable build artifacts.

Before loading any executable code the host copies the opened image to a
Linux memfd, hashes the copied bytes, verifies the pin, and seals the image
against writes and size changes. It then loads that same sealed image through
`/proc/self/fd`; replacing or modifying the installation path cannot change the
verified image. Keep plugin dependencies available through trusted system
loader paths; `$ORIGIN`-relative dependencies are not supported by this image
loading scheme. The pin covers the plugin image, not its transitive native
dependencies; avoid provider-specific dynamic dependencies where practical.

Libraries and their image file descriptors remain loaded until process exit,
including after descriptor validation fails. There is no hot reload or
`dlclose`; installation, replacement, removal, and upgrades require restart.
Missing plugins are not downloaded automatically.

**This is a trust boundary, not a sandbox.** Loading executes native constructors
before the descriptor can be inspected. A pinned plugin has the gateway's
privileges and can read credentials, access the network/filesystem, create
threads, terminate the process, or corrupt memory. Length/shape validation
detects ordinary contract errors, not forged pointers or malicious native code.
Malformed output allocations are deliberately not freed when their ownership
cannot be validated. Install only reviewed, administrator-trusted artifacts.
