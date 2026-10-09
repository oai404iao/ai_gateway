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

Do not replace the fast mocked UI suite under `web/console/e2e/`, backend
integration tests, paid opt-in smoke, or performance tooling with this suite.
