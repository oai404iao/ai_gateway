<p align="center">
  <img src="assets/logo.svg" width="96" height="96" alt="ai-gateway logo">
</p>

# ai-gateway

> Status: Current.

[简体中文](README.zh-CN.md) · [Documentation](docs/README.md) · [Releases](https://github.com/oai404iao/ai_gateway/releases)

With `ai-gateway`, applications use one OpenAI-compatible base URL to call multiple upstreams.
You host the gateway, issue its API keys, and choose the model names clients see. In the Console,
route each name to upstream models, grant access, and set usage prices.

## Run your first request

The [local quickstart](docs/user/quickstart.md) takes you from a fresh PostgreSQL database
to a Chat Completions reply through your own upstream. Bring its base URL, model ID and API key.

For a server, [deploy the release image with Docker Compose](docs/user/production-deployment.md).
The Rust binary serves the public API and a separate management Console; put the Console behind
an HTTPS reverse proxy. PostgreSQL is the default database. On a single Linux host, you can
also [use SQLite](docs/user/sqlite.md).

**Replacing an earlier installation?** This storage baseline requires a new database and a separate empty request-log spool. [Preserve the existing ledger before switching.](docs/user/upgrades.md)

## Connect clients and upstreams

| Client operation | Supported transport |
| --- | --- |
| Chat Completions | HTTP, including SSE streaming |
| Responses | HTTP, including SSE streaming, or WebSocket |
| Image generation and editing | Non-streaming HTTP; JSON for generation, multipart for edits |
| Standalone web search | Codex extension, non-streaming HTTP |

Each operation has its own [routing rules](docs/user/routing.md). The built-in `general` connector works with
OpenAI-compatible upstreams. To use a Codex account, [install and enable its native connector](docs/user/connector-plugins.md).

- [Call the public API](docs/user/data-plane.md) or check [protocol compatibility](docs/reference/openai-compatibility.md).
- [Manage users and keys](docs/user/console.md), [review request logs](docs/user/observability.md), and [set prices and soft quotas](docs/user/billing.md).
- [Protect credentials and backups](docs/user/security.md) before exposing the service.
- [Set up a development environment](docs/development/getting-started.md) to build or change the gateway.

## License

`ai-gateway` is licensed under the [GNU Affero General Public License v3.0 only](LICENSE)
(`AGPL-3.0-only`). If users interact with a modified version over a network, section 13
requires offering them its corresponding source. Third-party redistribution notices are in
[`LICENSES/`](LICENSES/) and [`web/console/NOTICES.md`](web/console/NOTICES.md).
