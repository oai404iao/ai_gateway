<p align="center">
  <img src="assets/logo.svg" width="96" height="96" alt="ai-gateway logo">
</p>

# ai-gateway

> **Status:** Current implementation, under active development.

[简体中文](README.zh-CN.md) · [Documentation](docs/README.md) · [Releases](https://github.com/oai404iao/ai_gateway/releases)

`ai-gateway` is a self-hosted, single-binary Rust gateway for OpenAI-compatible
LLM requests. Route traffic to configured upstreams, manage access through a
separate Console, and record usage with durable asynchronous accounting.

It supports Chat Completions, Responses over HTTP/SSE and WebSocket, non-streaming
Images generation and multipart edits, and a Codex standalone search extension.
API formats remain separate: the gateway does not translate between them.
See the [compatibility reference](docs/reference/openai-compatibility.md) for
supported operations and limits.

## Get started

Follow the [quickstart](docs/user/quickstart.md) to start PostgreSQL, configure
the Console, create the first administrator and route, and send a request.
An empty control plane can pass health checks but cannot proxy requests yet.

Choose another setup if it better fits your deployment:

| Goal | Guide |
| --- | --- |
| Deploy the released image on one host | [Production Compose](docs/user/production-deployment.md) |
| Use a local database file on Linux | [SQLite deployment and offline recovery](docs/user/sqlite.md) |
| Configure limits, storage, and capacity | [Production configuration](docs/user/production-configuration.md) |
| Connect a Codex subscription | [Install the connector plugin](docs/user/connector-plugins.md) |

PostgreSQL is the default backend. Source builds require `sqlite-backend` for
SQLite; official images and release binaries include it. The `general` connector
is built in. Codex requires a separately installed, explicitly enabled native
plugin from [ai-gateway-connectors](https://github.com/oai404iao/ai-gateway-connectors);
gateway distributions do not bundle it.

## Use and operate the gateway

- [Use the APIs and Console](docs/user/operations.md): client keys, routes,
  streaming, credentials, logs, and billing.
- [Check compatibility](docs/reference/README.md): accepted request fields,
  protocol behavior, and upstream-specific limitations.
- [Run securely](docs/user/security.md): listener exposure, credentials,
  trusted plugins, and recovery boundaries.

The public `/v1/*` listener uses client API keys. The Console API and optional
embedded React UI use a **separate** listener and JWT sessions. Production needs
no Node.js process; the gateway does not terminate TLS. Put the Console behind
an HTTPS reverse proxy.

Routing uses immutable in-memory snapshots rather than a per-request control-plane
database query. Upstream clients are reused, normal responses are streamed, and
requests are never retried after downstream response headers or bytes are sent.
RPM, concurrency, affinity, health, and WebSocket pools are process-local, not
cluster-wide coordination.

Request logs omit prompts, completions, complete headers, and credentials.
The database and backups still contain credential-sensitive control-plane data.
Quotas are soft checks, not prepaid cost reservations; see the
[operations guide](docs/user/operations.md) before relying on them for spending limits.

## Develop

Start with the [development setup](docs/development/getting-started.md), then
choose a topic from the [maintainer guide](docs/development/README.md).
Rust **1.97.1** is pinned in [`rust-toolchain.toml`](rust-toolchain.toml).
Console tooling requires Node.js 24 and pnpm 11.17.0.

With the development database configured and running:

```bash
cargo fmt --check
cargo clippy --locked --workspace --all-targets
cargo test --locked --workspace
```

For frontend commands, see [`web/console/README.md`](web/console/README.md).
Console API shapes come from [`console-v1.yaml`](docs/openapi/console-v1.yaml);
regenerate TypeScript types rather than editing them by hand.

Forwarding changes also require the [paid real-upstream smoke test](docs/development/real-upstream-smoke.md)
with explicit authorization. [Performance runs](docs/development/forwarding-performance.md)
are separately opt-in; neither belongs in routine setup.

## License

[GNU Affero General Public License v3.0 only](LICENSE) (`AGPL-3.0-only`).
If you modify the program and make it available to users over a network,
AGPL section 13 requires offering those users access to the corresponding source.
Third-party notices and redistribution material are in [`LICENSES/`](LICENSES/)
and [`web/console/NOTICES.md`](web/console/NOTICES.md).
