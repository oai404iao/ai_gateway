# Project system E2E

> Status: Current. Covers a real browser, pinned Codex HTTP/WS and Pi clients,
> crash/replay, and explicit spool-failure characterization against the production
> gateway and disposable PostgreSQL. Production admission changes remain separate.

See [the maintainer guide](../docs/development/system-e2e.md) for setup,
scenarios, isolation, safe reports, CI, and remaining phase-one work.

```bash
python3 -m venv target/e2e-venv
target/e2e-venv/bin/pip install --require-hashes -r e2e/requirements.txt
target/e2e-venv/bin/python -m unittest discover -s e2e -p 'test_*.py'
node --test e2e/browser-checks.test.mjs
target/e2e-venv/bin/python e2e/run.py
```

The runner requires a built embedded-Console binary, installed Console
dependencies/Chromium, Docker, OpenSSL, a C compiler, Node, Python 3.11+, Linux,
and the exact client versions in `clients.json`. It does not install dependencies or build implicitly.
It never reads local gateway configuration or real-upstream credentials.

Do not replace the fast mocked UI suite under `web/console/e2e/`, backend
integration tests, paid opt-in smoke, or performance tooling with this suite.
