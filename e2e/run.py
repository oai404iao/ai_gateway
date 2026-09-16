#!/usr/bin/env python3
"""Own the disposable database, production gateway, clients, and safe report."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from decimal import Decimal
from urllib.error import HTTPError, URLError
from urllib.request import Request, ProxyHandler, build_opener

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from mock.upstream import Upstream  # noqa: E402
from mock.websocket import WebSocketUpstream  # noqa: E402

POSTGRES = "postgres:18.4-alpine@sha256:9a8afca54e7861fd90fab5fdf4c42477a6b1cb7d293595148e674e0a3181de15"
CLIENTS = json.loads((ROOT / "e2e/clients.json").read_text())
CODEX_VERSION = CLIENTS["codex"]["version_output"]
MAX_LOG = 8 * 1024 * 1024
HTTP = build_opener(ProxyHandler({}))


def check(condition, message):
    if not condition:
        raise RuntimeError(message)


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def request(base, path, method="GET", body=None, token=None, etag=None):
    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if etag:
        headers["If-Match"] = etag
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = Request(base + path, method=method, headers=headers,
                  data=json.dumps(body).encode() if body is not None else None)
    try:
        with HTTP.open(req, timeout=10) as response:
            payload = response.read(MAX_LOG + 1)
            check(len(payload) <= MAX_LOG, "HTTP evidence limit exceeded")
            return json.loads(payload) if payload else None, response.headers
    except HTTPError as error:
        # Bodies can contain keys or echoed requests; only retain the error code.
        try:
            payload = json.loads(error.read(16384))
            code = payload.get("error", {}).get("code", "unknown")
        except (ValueError, AttributeError):
            code = "unknown"
        finally:
            error.close()
        raise RuntimeError(f"{method} {path}: HTTP {error.code} ({code})") from None


def wait_until(action, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if action():
            return
        time.sleep(0.2)
    raise TimeoutError("condition did not become ready")


def redact(text, values):
    for value in sorted(filter(None, values), key=len, reverse=True):
        text = text.replace(value, "[redacted]")
    return re.sub(r"(?i)Bearer\s+\S+", "Bearer [redacted]", text)


class Resources:
    def __init__(self, directory):
        self.directory = directory
        self.processes = []
        self.collectors = {}
        self.output_errors = []
        self.directories = []
        self.secret_values = []
        self.container = None
        self.env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(directory), "TMPDIR": str(directory),
            "LANG": "C.UTF-8", "NO_PROXY": "*",
        }

    def start(self, args, name, env=None, stdin=None, cwd=None):
        log = self.directory / f"{name}.log"
        process = subprocess.Popen(
            args, cwd=cwd or ROOT, env=env or self.env,
            stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True,
        )
        self.processes.append(process)

        def collect():
            try:
                with log.open("wb") as output, process.stdout:
                    remaining = MAX_LOG
                    while chunk := process.stdout.read(8192):
                        output.write(chunk[:remaining])
                        output.flush()
                        remaining -= len(chunk)
                        if remaining < 0:
                            raise RuntimeError(f"{name}: output limit exceeded")
            except (OSError, RuntimeError) as error:
                self.output_errors.append(str(error))
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

        collector = threading.Thread(target=collect, daemon=True)
        self.collectors[process.pid] = collector
        collector.start()
        if stdin is not None:
            process.stdin.write(stdin)
            process.stdin.close()
        return process, log

    def run(self, args, name, env=None, stdin=None, cwd=None, timeout=90):
        process, log = self.start(args, name, env, stdin, cwd)
        deadline = time.monotonic() + timeout
        while process.poll() is None:
            check(not self.output_errors, "; ".join(self.output_errors))
            if time.monotonic() >= deadline:
                raise TimeoutError(f"{name}: deadline exceeded")
            time.sleep(0.1)
        self.collectors[process.pid].join(timeout=5)
        check(not self.output_errors, "; ".join(self.output_errors))
        check(process.returncode == 0, f"{name}: exit {process.returncode}")
        check(log.stat().st_size <= MAX_LOG, f"{name}: output limit exceeded")
        return log.read_text(errors="replace")

    def docker(self, *args):
        result = subprocess.run(["docker", *args], capture_output=True, timeout=90)
        check(result.returncode == 0, f"docker {args[0]} failed")
        return result.stdout.decode().strip()

    def database(self):
        self.container = "ai-gateway-e2e-" + secrets.token_hex(8)
        password = secrets.token_hex(24)
        database = "ai_gateway_e2e_" + secrets.token_hex(8)
        self.database_name = database
        self.docker(
            "run", "--detach", "--name", self.container,
            "--label", "ai-gateway.system-e2e=true",
            "--publish", "127.0.0.1::5432",
            "--env", f"POSTGRES_PASSWORD={password}", "--env", f"POSTGRES_DB={database}",
            POSTGRES,
        )
        address = self.docker("port", self.container, "5432/tcp")
        check(address.startswith("127.0.0.1:"), "database must bind only loopback")

        def ready():
            return subprocess.run(
                ["docker", "exec", self.container, "pg_isready", "-h", "127.0.0.1",
                 "-U", "postgres", "-d", database],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5,
            ).returncode == 0

        wait_until(ready)
        return f"postgres://postgres:{password}@{address}/{database}", password

    def close(self):
        errors = []
        for process in reversed(self.processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                continue
            try:
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    pass
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                errors.append("child process cleanup failed")
        if self.container:
            try:
                self.docker("rm", "--force", "--volumes", self.container)
            except (RuntimeError, subprocess.TimeoutExpired):
                errors.append("database container cleanup failed")
        for collector in self.collectors.values():
            collector.join(timeout=5)
            if collector.is_alive():
                errors.append("output collector did not stop")
        for directory in self.directories:
            try:
                directory.cleanup()
            except OSError:
                errors.append("client directory cleanup failed")
        return errors + self.output_errors


def configure(resources, binary, database_url, public_port, console_port):
    private = resources.directory / "jwt-private.pem"
    public = resources.directory / "jwt-public.pem"
    resources.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(private)], "jwt")
    resources.run(["openssl", "pkey", "-in", str(private), "-pubout", "-out", str(public)], "jwt-public")
    config = resources.directory / "gateway.toml"
    quote = json.dumps
    config.write_text(f"""
[server]
host = "127.0.0.1"
port = {public_port}
shutdown_grace_period_seconds = 5
[database]
url = {quote(database_url)}
max_connections = 5
connect_timeout_seconds = 5
[upstream]
connect_timeout_seconds = 5
response_header_timeout_seconds = 15
stream_idle_timeout_seconds = 15
[runtime_config]
reload_interval_seconds = 30
[console]
enabled = true
ui_enabled = true
host = "127.0.0.1"
port = {console_port}
allowed_origins = ["http://localhost:{console_port}"]
[auth]
issuer = "system-e2e"
audience = "system-e2e-console"
key_id = "system-e2e"
access_token_ttl_seconds = 900
refresh_token_ttl_seconds = 3600
signing_key_path = {quote(str(private))}
verification_key_path = {quote(str(public))}
[request_limits]
image_edit_spool_directory = {quote(str(resources.directory / "images"))}
[request_logging]
spool_directory = {quote(str(resources.directory / "spool"))}
settlement_interval_milliseconds = 100
shutdown_drain_seconds = 5
[request_retry]
enabled = false
[observability]
filter = "ai_gateway=info,tower_http=warn"
""")
    config.chmod(0o600)
    password = secrets.token_urlsafe(24)
    resources.run([
        str(binary), "bootstrap-admin", "--config", str(config),
        "--email", "system-e2e@example.test", "--display-name", "System E2E", "--password-stdin",
    ], "bootstrap", stdin=(password + "\n").encode())
    gateway, _ = resources.start([str(binary), str(config)], "gateway")
    resources.gateway = gateway
    console = f"http://localhost:{console_port}"

    def ready():
        check(gateway.poll() is None, "gateway exited before readiness")
        try:
            request(f"http://127.0.0.1:{public_port}", "/health")
            return True
        except URLError:
            return False

    wait_until(ready)
    return console, password


def seed(console, password, upstream):
    login, _ = request(console, "/console/v1/auth/login", "POST", {
        "email": "system-e2e@example.test", "password": password,
    })
    token = login["access_token"]

    def api(path, method="GET", body=None, etag=None):
        return request(console, "/console/v1" + path, method, body, token, etag)

    user = login["user"]["id"]
    _, headers = api(f"/users/{user}")
    api(f"/users/{user}", "PATCH", {"balance_amount": "100"}, headers["ETag"])
    group, _ = api("/routing/channel-groups", "POST", {
        "name": "system-e2e", "api_format": "open_ai_responses", "enabled": True,
    })
    channel, _ = api("/routing/channels", "POST", {
        "channel_group_id": group["id"], "api_format": "open_ai_responses",
        "name": "system-e2e", "base_url": upstream, "enabled": True,
        "upstream_auth_kind": "none", "available_models": ["e2e-before", "e2e-wire"],
    })
    model, _ = api("/models", "POST", {
        "source_model_id": "e2e-client", "display_name": "System E2E model", "enabled": True,
        "price_unit_tokens": 1000000, "input_unit_price": "1",
        "cached_input_unit_price": "0", "cache_write_unit_price": "0", "output_unit_price": "2",
        "price_effective_at": "2026-01-01T00:00:00Z",
    })
    parent, _ = api("/routing/model-rules", "POST", {"model_id": model["id"]})
    protocol, _ = api(f"/routing/model-rules/{parent['id']}/protocols", "POST", {
        "api_format": "open_ai_responses",
    })
    path = f"/routing/model-rules/{parent['id']}/protocols/{protocol['id']}"
    _, headers = api(path)
    api(path, "PUT", {
        "description": "Before browser edit", "enabled": True,
        "routing_tiers": [{
            "priority": 0, "selection_strategy": "weighted_round_robin",
            "candidates": [{"channel_id": channel["id"], "upstream_model": "e2e-before", "weight": 1}],
        }],
    }, headers["ETag"])
    key, _ = api("/api-keys", "POST", {
        "user_id": user, "name": "system-e2e", "allowed_api_formats": ["open_ai_responses"],
        "permissions": ["proxy"], "allowed_group_ids": [group["id"]], "allowed_channel_ids": [],
    })
    return {
        "console": console, "password": password, "token": token, "user_id": user,
        "api_key": key["secret"], "api_key_id": key["id"], "channel_id": channel["id"],
        "channel_group_id": group["id"], "protocol_path": path,
    }


def set_upstream(data, url, websocket=False):
    def api(path, method="GET", body=None, etag=None):
        return request(data["console"], "/console/v1" + path, method, body, data["token"], etag)

    if websocket:
        settings, headers = api("/system/settings")
        settings.pop("updated_at", None)
        settings["websocket"]["enabled"] = True
        api("/system/settings", "PUT", settings, headers["ETag"])
        _, headers = api(f"/users/{data['user_id']}")
        api(f"/users/{data['user_id']}", "PATCH", {"websocket_enabled": True}, headers["ETag"])
    path = f"/routing/channels/{data['channel_id']}"
    _, headers = api(path)
    api(path, "PUT", {
        "channel_group_id": data["channel_group_id"], "api_format": "open_ai_responses",
        "name": "system-e2e", "base_url": url, "enabled": True,
        "upstream_auth_kind": "none", "available_models": ["e2e-before", "e2e-wire"],
        "supports_websocket": websocket,
    }, headers["ETag"])


def run_codex(resources, binary, data, marker, websocket=False):
    name = "codex-ws" if websocket else "codex-http"
    version = resources.run([binary, "--version"], name + "-version").strip().splitlines()[-1]
    check(version == CODEX_VERSION, f"requires {CODEX_VERSION}, got {version}")
    home = resources.directory / (name + "-home")
    # Keep cwd outside any repository so the CLI cannot inherit its AGENTS or
    # project configuration. CODEX_HOME stays outside /tmp for helper binaries.
    directory = tempfile.TemporaryDirectory(prefix="ai-gateway-cli-")
    resources.directories.append(directory)
    work = Path(directory.name)
    home.mkdir()
    (work / "marker.txt").write_text(marker)
    (home / "config.toml").write_text(f"""
model = "e2e-client"
model_provider = "system_e2e"
approval_policy = "never"
sandbox_mode = "read-only"
[model_providers.system_e2e]
name = "Loopback fixture"
base_url = "{data['public']}/v1"
env_key = "SYSTEM_E2E_KEY"
wire_api = "responses"
supports_websockets = {str(websocket).lower()}
request_max_retries = 0
stream_max_retries = 0
""")
    output = work / "answer.txt"
    resources.run([
        binary, "exec", "--ephemeral", "--skip-git-repo-check", "--output-last-message", str(output),
        "Read marker.txt with the shell tool. Then reply E2E_TOOL_OK. Do not use network or write files.",
    ], name, env={**resources.env, "CODEX_HOME": str(home), "SYSTEM_E2E_KEY": data["api_key"]},
        cwd=work)
    check(output.read_text().strip() == "E2E_TOOL_OK", "CLI final output mismatch")
    return version


def run_pi(resources, binary, data, marker):
    env = {**resources.env, "PI_OFFLINE": "1", "PI_TELEMETRY": "0"}
    version = resources.run([binary, "--version"], "pi-version", env=env).strip()
    check(version == CLIENTS["pi"]["version_output"], "Pi version mismatch")
    home = resources.directory / "pi-home"
    home.mkdir()
    directory = tempfile.TemporaryDirectory(prefix="ai-gateway-pi-")
    resources.directories.append(directory)
    work = Path(directory.name)
    (work / "marker.txt").write_text(marker)
    (home / "models.json").write_text(json.dumps({"providers": {"system-e2e": {
        "baseUrl": data["public"] + "/v1", "api": "openai-responses",
        "apiKey": "$SYSTEM_E2E_KEY", "models": [{"id": "e2e-client"}],
    }}}))
    output = resources.run([
        binary, "--mode", "json", "--no-session", "--offline", "--no-approve",
        "--no-context-files", "--no-extensions", "--no-skills", "--no-prompt-templates",
        "--no-themes", "--provider", "system-e2e", "--model", "e2e-client",
        "--tools", "read", "--thinking", "off",
        "Read marker.txt with the read tool, then reply E2E_TOOL_OK.",
    ], "pi", env={**env, "PI_CODING_AGENT_DIR": str(home), "SYSTEM_E2E_KEY": data["api_key"]}, cwd=work)
    validate_pi_output(output, marker)
    return version


def validate_pi_output(output, marker):
    events = [json.loads(line) for line in output.split("\n") if line]
    tools = [event for event in events if event["type"] == "tool_execution_end"]
    check(len(tools) == 1 and tools[0]["isError"] is False, "Pi tool execution failed")
    check(tools[0]["toolName"] == "read" and marker in json.dumps(tools[0]["result"]), "Pi read evidence missing")
    check(tools[0]["toolCallId"].split("|", 1)[0] == "call_system_e2e", "Pi tool ID mismatch")
    check(sum(event["type"] == "agent_end" for event in events) == 1, "Pi agent end missing")
    finals = [event["message"] for event in events if event["type"] == "message_end"
              and event["message"]["role"] == "assistant"]
    check(finals and finals[-1]["stopReason"] == "stop", "Pi terminal event missing")
    check(any(part.get("text") == "E2E_TOOL_OK" for part in finals[-1]["content"]), "Pi final text mismatch")


def verify_settlement(data, expected_count, zero_count=0, protocols=None):
    def api(path):
        return request(data["console"], "/console/v1" + path, token=data["token"])[0]

    settled = []

    def ready():
        nonlocal settled
        logs = api(f"/request-logs?api_key_id={data['api_key_id']}")
        if len(logs) != expected_count or any(log["billed_at"] is None for log in logs):
            return False
        settled = logs
        return True

    wait_until(ready)
    check(sum(log["input_tokens"] == 0 and log["output_tokens"] == 0 for log in settled) == zero_count,
          "zero-usage count mismatch")
    if protocols:
        check(Counter(log["request_protocol"] for log in settled) == Counter(protocols),
              "unexpected request transport or HTTP fallback")
    for log in settled:
        check(log["outcome"] == "succeeded" and log["response_status_code"] == 200, "request failed")
        check(log["client_model"] == "e2e-client" and log["upstream_model"] == "e2e-wire", "log model mismatch")
        check(log["channel_id"] == data["channel_id"], "log channel mismatch")
        zero = log["input_tokens"] == 0 and log["output_tokens"] == 0
        check(zero or (log["input_tokens"] == 5 and log["output_tokens"] == 2), "usage mismatch")
        check(Decimal(log["cost_amount"]) == Decimal("0" if zero else "0.000009"), "cost mismatch")
    expected = Decimal("0.000009") * (expected_count - zero_count)
    check(Decimal(api("/me")["balance_amount"]) == Decimal("100") - expected, "balance mismatch")
    check(Decimal(api(f"/api-keys/{data['api_key_id']}")["quota_used_amount"]) == expected, "key usage mismatch")
    return {"logs": len(settled), "cost": str(expected), "billed": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/ai-gateway")
    parser.add_argument("--codex", default=shutil.which("codex"))
    parser.add_argument("--pi", default=shutil.which("pi"))
    parser.add_argument("--output", type=Path, help="new directory under target/system-e2e")
    args = parser.parse_args()
    output = (args.output or ROOT / "target/system-e2e" / secrets.token_hex(8)).resolve()
    check(output.is_relative_to(ROOT / "target/system-e2e"), "output must be under target/system-e2e")
    os.umask(0o077)
    output.mkdir(parents=True, exist_ok=False)
    report = {"version": 1, "status": "failed", "stage": "preflight", "scenarios": []}
    secret_values = []
    interrupted = False

    def interrupt(_signum, _frame):
        raise InterruptedError("system E2E interrupted")

    previous = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGINT, signal.SIGTERM)}
    with tempfile.TemporaryDirectory(prefix="system-e2e-private-", dir=ROOT / "target") as temporary:
        resources = Resources(Path(temporary))
        try:
            check(args.binary.is_file(), "build embedded-console-ui binary first")
            check(args.codex is not None, "install the pinned Codex CLI")
            check(args.pi is not None, "install the pinned Pi CLI")
            for command in ("docker", "node", "openssl", "cc"):
                check(shutil.which(command), f"{command} is required")
            report["source_commit"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT).decode().strip()
            report["working_tree_dirty"] = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
            with args.binary.open("rb") as binary:
                report["binary_sha256"] = hashlib.file_digest(binary, "sha256").hexdigest()
            harness = hashlib.sha256()
            for area in ("e2e", "mock"):
                for path in sorted((ROOT / area).rglob("*")):
                    if path.suffix in (".py", ".mjs", ".json", ".c", ".txt"):
                        harness.update(str(path.relative_to(ROOT)).encode())
                        harness.update(path.read_bytes())
            report["harness_sha256"] = harness.hexdigest()
            report["postgres_image"] = POSTGRES
            report["stage"] = "database"
            database, db_password = resources.database()
            secret_values.extend([database, db_password])
            public_port, console_port = free_port(), free_port()
            check(public_port != console_port, "listener port collision")
            report["stage"] = "bootstrap"
            console, password = configure(resources, args.binary.resolve(), database, public_port, console_port)
            secret_values.append(password)
            marker = secrets.token_hex(24)
            with Upstream(marker) as upstream:
                report["stage"] = "provision"
                data = seed(console, password, upstream.url)
                secret_values.extend([data["token"], data["api_key"]])
                data["public"] = f"http://127.0.0.1:{public_port}"
                report["stage"] = "browser"
                browser_env = {
                    **resources.env,
                    "PLAYWRIGHT_BROWSERS_PATH": os.environ.get(
                        "PLAYWRIGHT_BROWSERS_PATH", str(Path.home() / ".cache/ms-playwright")),
                }
                browser = resources.run(["node", str(ROOT / "e2e/browser.mjs")], "browser",
                                        env=browser_env, stdin=json.dumps(data).encode())
                report["scenarios"].append(json.loads(browser))
                verify_settlement(data, 1)
                report["stage"] = "codex"
                report["codex_version"] = run_codex(resources, args.codex, data, marker)
                check(upstream.scenario.tool_completed, "tool loop not completed")
                check(not upstream.scenario.errors, "fixture rejected a request")
                check(len(upstream.scenario.evidence) == 3, "unexpected upstream request count")
                report["scenarios"].append({"id": "responses-tool-cycle", "status": "passed"})
                report["upstream"] = upstream.scenario.evidence
                report["stage"] = "settlement"
                report["settlement"] = verify_settlement(data, 3)
            report["stage"] = "codex-websocket"
            with WebSocketUpstream(marker) as upstream:
                set_upstream(data, upstream.url, websocket=True)
                run_codex(resources, args.codex, data, marker, websocket=True)
                check(upstream.scenario.tool_completed and not upstream.errors
                      and not upstream.scenario.errors, "WS tool/continuation contract failed")
                evidence = upstream.scenario.evidence
                check(len(evidence) == 2 and len({entry["connection"] for entry in evidence}) == 1,
                      "WS tool cycle did not use one connection")
                check(evidence[1]["previous_response_id"] == "resp_e2e_0", "WS continuation missing")
                warmups = upstream.warmups
                report["upstream"].extend(evidence)
                report["scenarios"].append({"id": "codex-ws-tool-cycle", "status": "passed",
                                            "warmups": warmups})
            report["stage"] = "pi-responses"
            with Upstream(marker) as upstream:
                set_upstream(data, upstream.url)
                report["pi_version"] = run_pi(resources, args.pi, data, marker)
                check(upstream.scenario.tool_completed and not upstream.scenario.errors
                      and len(upstream.scenario.evidence) == 2, "Pi upstream evidence missing")
                report["upstream"].extend(upstream.scenario.evidence)
                report["scenarios"].append({"id": "pi-responses-tool-cycle", "status": "passed"})
            report["stage"] = "settlement"
            report["settlement"] = verify_settlement(
                data, 7 + warmups, warmups,
                ["non_stream"] + ["sse"] * 4 + ["websocket"] * (2 + warmups),
            )
            report["stage"] = "durability-faults"
            from faults import exercise_faults
            faults, settlement = exercise_faults(
                resources, args.binary.resolve(), data, 7 + warmups, warmups,
                ["non_stream"] + ["sse"] * 4 + ["websocket"] * (2 + warmups),
            )
            report["scenarios"].extend(faults)
            report["settlement"] = settlement
            report["status"] = "passed"
        except Exception as error:
            interrupted = isinstance(error, (InterruptedError, KeyboardInterrupt))
            report["error"] = redact(str(error), secret_values + resources.secret_values)[:4000]
        finally:
            for sig in previous:
                signal.signal(sig, signal.SIG_IGN)
            cleanup = resources.close()
            report["cleanup"] = {"status": "failed" if cleanup else "passed", "errors": cleanup}
            if cleanup:
                report["status"] = "failed"
            if report["status"] != "passed":
                for path in resources.directory.glob("*.log"):
                    with path.open("rb") as file:
                        file.seek(max(0, path.stat().st_size - 16384))
                        text = file.read(16384).decode(errors="replace")
                    (output / path.name).write_text(redact(text, secret_values + resources.secret_values))
            (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
            for sig, handler in previous.items():
                signal.signal(sig, handler)
    print(f"system E2E: {report['status']} — {output / 'report.json'}")
    if "error" in report:
        print(f"{report['stage']}: {report['error']}", file=sys.stderr)
    return 130 if interrupted else (0 if report["status"] == "passed" else 1)


if __name__ == "__main__":
    sys.exit(main())
