"""Isolated crash/replay and process-local durable admission fault acceptance."""

import signal
import shutil
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from urllib.error import URLError

from run import ROOT, check, request, set_upstream, verify_settlement, wait_until
from mock.upstream import Upstream


JOURNAL = Path("spool/journal")


def stop_gateway(resources):
    process = resources.gateway
    process.send_signal(signal.SIGKILL)
    process.wait(timeout=5)
    resources.collectors[process.pid].join(timeout=5)
    check(process.returncode == -signal.SIGKILL, "gateway was not killed at requested boundary")


def start_gateway(resources, binary, data, name, extra_env=None):
    resources.gateway, _ = resources.start(
        [str(binary), str(resources.directory / "gateway.toml")],
        name, env={**resources.env, **(extra_env or {})},
    )

    def ready():
        check(resources.gateway.poll() is None, "restarted gateway exited")
        try:
            request(data["public"], "/health")
            return True
        except URLError:
            return False

    wait_until(ready)


def build_injector(resources):
    library = resources.directory / "spool-fault.so"
    resources.run([
        "cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
        str(ROOT / "e2e/spool-fault.c"), "-o", str(library),
    ], "build-fault-injector")
    return library


def restore_postgres_pair(resources):
    source = resources.database_name
    restored = source + "_restored"
    def docker(*args):
        return resources.docker("exec", resources.container, *args)
    archive = "/var/lib/postgresql/paired.dump"
    docker("pg_dump", "-U", "postgres", "-Fc", "-f", archive, source)
    spool = resources.directory / "spool"
    snapshot = resources.directory / "stopped-spool"
    spool.rename(snapshot)
    shutil.copytree(snapshot, spool)
    docker("createdb", "-U", "postgres", "-T", "template0", restored)
    docker("pg_restore", "-U", "postgres", "--exit-on-error", "-d", restored, archive)
    config = resources.directory / "gateway.toml"
    text = config.read_text()
    check(text.count(f'/{source}"') == 1, "test database URL not found")
    config.write_text(text.replace(f'/{source}"', f'/{restored}"'))
    resources.database_name = restored


def exercise_faults(resources, binary, data, protocols, warmup_id):
    def api(path, method="GET", body=None):
        return request(data["console"], "/console/v1" + path, method, body, data["token"])[0]

    def dispatch(key=None):
        result, _ = request(data["public"], "/v1/responses", "POST", {
            "model": "e2e-client", "input": "Reply E2E_TEXT_OK", "stream": False,
        }, token=key or data["api_key"])
        check(result["status"] == "completed", "fixture request did not complete")

    def denied(key=None):
        try:
            dispatch(key)
        except RuntimeError as error:
            check(str(error) == "POST /v1/responses: HTTP 503 (request_log_unavailable)",
                  "unexpected admission error")
        else:
            raise AssertionError("unavailable journal allowed dispatch")

    results = []
    journal = resources.directory / JOURNAL / "events.log"
    checkpoint = journal.with_name("checkpoint")
    admissions = journal.with_name("admissions")
    with Upstream("unused", secret=data["upstream_rotated_secret"]) as upstream:
        set_upstream(data, upstream.url)
        with resources.database_outage():
            dispatch()
            old_checkpoint = checkpoint.read_bytes() if checkpoint.exists() else None
            offset = int.from_bytes(old_checkpoint or b"\0" * 8, "little")
            wait_until(lambda: journal.stat().st_size > offset)
            stop_gateway(resources)
        protocols.append("non_stream")
        start_gateway(resources, binary, data, "gateway-recovered")
        verify_settlement(data, protocols, warmup_id)
        results.append({"id": "db-outage-kill-replay", "status": "passed"})

        # Rewind only the private checkpoint with the gateway stopped. This
        # simulates DB commit succeeding before the local checkpoint is durable.
        stop_gateway(resources)
        if old_checkpoint is None:
            checkpoint.unlink(missing_ok=True)
        else:
            checkpoint.write_bytes(old_checkpoint)
        if resources.backend == "postgres":
            restore_postgres_pair(resources)
        start_gateway(resources, binary, data, "gateway-duplicate-replay")
        wait_until(lambda: api("/system/load")["request_log"]["spool_pending_bytes"] == 0)
        wait_until(resources.ingress_empty)
        verify_settlement(data, protocols, warmup_id)
        check(upstream.post_count == 1, "recovery redispatched a model request")
        results.append({"id": "duplicate-replay-no-double-charge", "status": "passed"})
        if resources.backend == "postgres":
            results.append({"id": "postgres-offline-paired-restore", "status": "passed"})

    library = build_injector(resources)
    control = resources.directory / "spool-fault-mode"
    env = {
        "LD_PRELOAD": str(library),
        "E2E_SPOOL_FAULT_PATH": str(admissions) + "/",
        "E2E_SPOOL_FAULT_CONTROL": str(control),
    }
    for mode in ("ENOSPC", "EACCES", "EIO_SYNC"):
        with Upstream("unused", secret=data["upstream_rotated_secret"]) as upstream:
            set_upstream(data, upstream.url)
            key = api("/api-keys", "POST", {
                "user_id": data["user_id"], "name": f"fault-{mode}",
                "permissions": ["proxy"],
                "allowed_group_ids": [data["channel_group_id"]], "allowed_channel_ids": [],
            })
            resources.secret_values.append(key["secret"])
            before = api("/me")["balance_amount"]
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-{mode}", env)
            control.write_text(mode)
            denied(key["secret"])
            control.unlink()
            denied(key["secret"])
            check(api(f"/request-logs?api_key_id={key['id']}") == [], "failed append unexpectedly produced a log")
            check(api("/me")["balance_amount"] == before, "failed append unexpectedly changed balance")
            check(upstream.post_count == 0, "failed admission dispatched upstream")
            results.append({
                "id": f"spool-write-{mode.lower()}", "status": "passed",
                "dispatches": 0, "durable_logs": 0, "writer_latched_until_restart": True,
            })
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-after-{mode}")
            set_upstream(data, upstream.url)
            dispatch()
            protocols.append("non_stream")
            verify_settlement(data, protocols, warmup_id)

            stop_gateway(resources)
            terminal_env = {**env, "E2E_SPOOL_FAULT_PATH": str(journal)}
            start_gateway(resources, binary, data, f"gateway-terminal-{mode}", terminal_env)
            control.write_text(mode)
            dispatch()
            wait_until(lambda: api("/system/load")["request_log"]["spool_append_failures_total"] == 1)
            control.unlink()
            denied()
            check(upstream.post_count == 2, "latched terminal failure allowed redispatch")
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-terminal-replay-{mode}")
            protocols.append("non_stream")
            verify_settlement(data, protocols, warmup_id)
            check(upstream.post_count == 2, "terminal slot replay redispatched a model request")
            results.append({
                "id": f"terminal-slot-replay-{mode.lower()}", "status": "passed",
                "replayed_without_redispatch": True,
            })

    gate = threading.Event()
    with (Upstream("unused", secret=data["upstream_rotated_secret"], response_gate=gate) as upstream,
          ThreadPoolExecutor(max_workers=1) as client):
        set_upstream(data, upstream.url)
        before_ids = {path.name for path in admissions.glob("*.json")}
        future = client.submit(dispatch)
        try:
            wait_until(lambda: len(upstream.scenario.evidence) == 1)
            intents = [path for path in admissions.glob("*.json") if path.name not in before_ids]
            check(len(intents) == 1, "dispatched request lacks exactly one durable intent")
            stop_gateway(resources)
        finally:
            gate.set()
        try:
            future.result(timeout=15)
        except (OSError, RuntimeError):
            pass
        else:
            raise AssertionError("request completed before the forced pre-terminal crash")
        start_gateway(resources, binary, data, "gateway-unknown-pending")
        check(intents[0].exists(), "unknown request intent was lost on restart")
        check(intents[0].with_suffix(".slot").stat().st_size == 0,
              "unknown request retained unused terminal allocation")
        verify_settlement(data, protocols, warmup_id)
        check(upstream.post_count == 1, "unknown request was redispatched")
        dispatch()
        protocols.append("non_stream")
        verify_settlement(data, protocols, warmup_id)
        results.append({
            "id": "kill-before-terminal-pending", "status": "passed",
            "unknown_usage_retained": True, "new_dispatch_allowed": True,
        })
    return results, verify_settlement(data, protocols, warmup_id)
