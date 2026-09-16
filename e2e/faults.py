"""Isolated crash/replay and process-local spool write-failure characterization."""

import signal
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

    results = []
    with Upstream("unused") as upstream:
        set_upstream(data, upstream.url)
        resources.docker("pause", resources.container)
        try:
            dispatch()
            journal = resources.directory / "spool/events.log"
            checkpoint = resources.directory / "spool/checkpoint"
            old_checkpoint = checkpoint.read_bytes() if checkpoint.exists() else None
            offset = int.from_bytes(old_checkpoint or b"\0" * 8, "little")
            wait_until(lambda: journal.stat().st_size > offset)
            stop_gateway(resources)
        finally:
            resources.docker("unpause", resources.container)
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
        wait_until(lambda: resources.docker(
            "exec", resources.container, "psql", "-U", "postgres", "-d", resources.database_name,
            "-Atc", "SELECT count(*) FROM request_log_ingest",
        ) == "0")
        verify_settlement(data, count, warmups, protocols)
        check(len(upstream.scenario.evidence) == 1, "recovery redispatched a model request")
        results.append({"id": "duplicate-replay-no-double-charge", "status": "passed"})

    library = build_injector(resources)
    control = resources.directory / "spool-fault-mode"
    env = {
        "LD_PRELOAD": str(library),
        "E2E_SPOOL_FAULT_PATH": str(resources.directory / "spool/events.log"),
        "E2E_SPOOL_FAULT_CONTROL": str(control),
    }
    for mode in ("ENOSPC", "EACCES"):
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
            dispatch(key["secret"])
            wait_until(lambda: api("/system/load")["request_log"]["spool_append_failures_total"] == 1)
            control.unlink()
            dispatch(key["secret"])
            wait_until(lambda: api("/system/load")["request_log"]["spool_append_failures_total"] == 2)
            check(api(f"/request-logs?api_key_id={key['id']}") == [], "failed append unexpectedly produced a log")
            check(api("/me")["balance_amount"] == before, "failed append unexpectedly changed balance")
            check(len(upstream.scenario.evidence) == 2, "dispatch count did not match append failures")
            results.append({
                "id": f"spool-write-{mode.lower()}", "status": "passed", "kind": "characterization",
                "observed_gap": "ordinary requests still dispatch after terminal append failure",
                "failed_appends": 2, "durable_logs": 0, "writer_latched_until_restart": True,
            })
            stop_gateway(resources)
            start_gateway(resources, binary, data, f"gateway-after-{mode}")
            set_upstream(data, upstream.url)
            dispatch()
            count += 1
            protocols.append("non_stream")
            verify_settlement(data, count, warmups, protocols)
    return results, verify_settlement(data, count, warmups, protocols)
