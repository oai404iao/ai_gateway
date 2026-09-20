"""Isolated crash/replay and process-local durable admission fault acceptance."""

import signal
import threading
from concurrent.futures import ThreadPoolExecutor
from urllib.error import URLError

from run import ROOT, check, request, set_upstream, verify_settlement, wait_until
from mock.upstream import Upstream


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


def exercise_faults(resources, binary, data, count, warmups, protocols):
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
    with Upstream("unused") as upstream:
        set_upstream(data, upstream.url)
        with resources.database_outage():
            dispatch()
            journal = resources.directory / "spool/events.log"
            checkpoint = resources.directory / "spool/checkpoint"
            old_checkpoint = checkpoint.read_bytes() if checkpoint.exists() else None
            offset = int.from_bytes(old_checkpoint or b"\0" * 8, "little")
            wait_until(lambda: journal.stat().st_size > offset)
            stop_gateway(resources)
        count += 1
        protocols = [*protocols, "non_stream"]
        start_gateway(resources, binary, data, "gateway-recovered")
        verify_settlement(data, count, warmups, protocols)
        results.append({"id": "db-outage-kill-replay", "status": "passed"})

        # Rewind only the private checkpoint with the gateway stopped. This
        # simulates DB commit succeeding before the local checkpoint is durable.
        stop_gateway(resources)
        if old_checkpoint is None:
            checkpoint.unlink(missing_ok=True)
        else:
            checkpoint.write_bytes(old_checkpoint)
        start_gateway(resources, binary, data, "gateway-duplicate-replay")
        wait_until(lambda: api("/system/load")["request_log"]["spool_pending_bytes"] == 0)
        wait_until(resources.ingress_empty)
        verify_settlement(data, count, warmups, protocols)
        check(len(upstream.scenario.evidence) == 1, "recovery redispatched a model request")
        results.append({"id": "duplicate-replay-no-double-charge", "status": "passed"})

    library = build_injector(resources)
    control = resources.directory / "spool-fault-mode"
    env = {
        "LD_PRELOAD": str(library),
        "E2E_SPOOL_FAULT_PATH": str(resources.directory / "spool/admissions") + "/",
        "E2E_SPOOL_FAULT_CONTROL": str(control),
    }
    for mode in ("ENOSPC", "EACCES", "EIO_SYNC"):
        with Upstream("unused") as upstream:
            set_upstream(data, upstream.url)
            key = api("/api-keys", "POST", {
                "user_id": data["user_id"], "name": f"fault-{mode}",
                "allowed_api_formats": ["open_ai_responses"], "permissions": ["proxy"],
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
            check(len(upstream.scenario.evidence) == 0, "failed admission dispatched upstream")
            results.append({
                "id": f"spool-write-{mode.lower()}", "status": "passed",
                "dispatches": 0, "durable_logs": 0, "writer_latched_until_restart": True,
            })
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-after-{mode}")
            set_upstream(data, upstream.url)
            dispatch()
            count += 1
            protocols.append("non_stream")
            verify_settlement(data, count, warmups, protocols)

            stop_gateway(resources)
            terminal_env = {**env, "E2E_SPOOL_FAULT_PATH": str(resources.directory / "spool/events.log")}
            start_gateway(resources, binary, data, f"gateway-terminal-{mode}", terminal_env)
            control.write_text(mode)
            dispatch()
            wait_until(lambda: api("/system/load")["request_log"]["spool_append_failures_total"] == 1)
            control.unlink()
            denied()
            check(len(upstream.scenario.evidence) == 2, "latched terminal failure allowed redispatch")
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-terminal-replay-{mode}")
            count += 1
            protocols.append("non_stream")
            verify_settlement(data, count, warmups, protocols)
            results.append({
                "id": f"terminal-slot-replay-{mode.lower()}", "status": "passed",
                "replayed_without_redispatch": True,
            })

    gate = threading.Event()
    with Upstream("unused", response_gate=gate) as upstream, ThreadPoolExecutor(max_workers=1) as client:
        set_upstream(data, upstream.url)
        pending_directory = resources.directory / "spool/admissions"
        before_ids = {path.name for path in pending_directory.glob("*.json")}
        future = client.submit(dispatch)
        try:
            wait_until(lambda: len(upstream.scenario.evidence) == 1)
            intents = [path for path in pending_directory.glob("*.json") if path.name not in before_ids]
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
        verify_settlement(data, count, warmups, protocols)
        check(len(upstream.scenario.evidence) == 1, "unknown request was redispatched")
        dispatch()
        count += 1
        protocols.append("non_stream")
        verify_settlement(data, count, warmups, protocols)
        results.append({
            "id": "kill-before-terminal-pending", "status": "passed",
            "unknown_usage_retained": True, "new_dispatch_allowed": True,
        })
    return results, verify_settlement(data, count, warmups, protocols)
