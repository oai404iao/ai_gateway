# Project system E2E

> Status: Partial implementation. The first slice covers a real browser and a
> pinned Codex CLI against the production gateway and disposable PostgreSQL.

See [the maintainer guide](../docs/development/system-e2e.md) for setup,
scenarios, isolation, safe reports, CI, and remaining phase-one work.

```bash
python3 -m unittest discover -s e2e -p 'test_*.py'
node --test e2e/browser-checks.test.mjs
python3 e2e/run.py
```

The runner requires a built embedded-Console binary, installed Console
dependencies/Chromium, Docker, OpenSSL, Node, Python 3.11+, and the exact client
version in `clients.json`. It does not install dependencies or build implicitly.
It never reads local gateway configuration or real-upstream credentials.

Do not replace the fast mocked UI suite under `web/console/e2e/`, backend
integration tests, paid opt-in smoke, or performance tooling with this suite.
