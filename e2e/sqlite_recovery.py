"""Real binary ownership, paired backup, corruption rejection and restored settlement."""

import json
import shutil
from run import check, verify_settlement
from faults import start_gateway, stop_gateway


def rejected(resources, args, name, stdin=None, extra_env=None):
    process, _ = resources.start(args, name, stdin=stdin, env={**resources.env, **(extra_env or {})})
    process.wait(timeout=30)
    resources.collectors[process.pid].join(timeout=5)
    check(process.returncode != 0, f"{name}: unsafe operation was accepted")


def exercise_backup(resources, binary, data, count, warmups):
    config = resources.directory / "gateway.toml"
    snapshot = resources.directory / "snapshot"
    backup = [str(binary), "backup-sqlite", "--config", str(config), "--destination", str(snapshot)]
    rejected(resources, backup, "reject-live-backup")
    check(not snapshot.exists(), "live backup wrote a destination")
    rejected(resources, [str(binary), "reset-admin-password", "--config", str(config),
                         "--email", "system-e2e@example.test", "--password-stdin"],
             "reject-live-administrator-command", stdin=b"test-only-never-applied-password-1234\n")
    stop_gateway(resources)
    unknown = sorted(path.name for path in (resources.directory / "spool/admissions").glob("*.json"))
    check(unknown, "backup test needs the unresolved pre-terminal request")
    resources.run(backup, "backup-sqlite")
    manifest = json.loads((snapshot / "manifest.json").read_text())
    check(manifest["version"] == 1, "unexpected snapshot version")
    control = resources.directory / "restore-fault-mode"
    control.write_text("ENOSPC")
    interrupted = resources.directory / "interrupted-restore"
    rejected(resources, [str(binary), "restore-sqlite", "--source", str(snapshot),
                         "--destination", str(interrupted)], "reject-incomplete-restore",
             extra_env={"LD_PRELOAD": str(resources.directory / "spool-fault.so"),
                        "E2E_SPOOL_FAULT_PATH": str(resources.directory) + "/",
                        "E2E_SPOOL_FAULT_CONTROL": str(control)})
    control.unlink()
    check(not interrupted.exists(), "incomplete restore published its destination")
    restored = resources.directory / "restored"
    restore = [str(binary), "restore-sqlite", "--source", str(snapshot), "--destination", str(restored)]
    resources.run(restore, "restore-sqlite")
    rejected(resources, restore, "reject-existing-restore")
    corrupt = resources.directory / "corrupt-snapshot"
    shutil.copytree(snapshot, corrupt)
    with (corrupt / "database/gateway.sqlite.identity").open("ab") as stream:
        stream.write(b"x")
    failed = resources.directory / "failed-restore"
    rejected(resources, [str(binary), "restore-sqlite", "--source", str(corrupt),
                         "--destination", str(failed)], "reject-corrupt-snapshot")
    check(not failed.exists(), "corrupt snapshot created a restore destination")
    text = config.read_text()
    text = text.replace("sqlite://" + str(resources.database_file),
                        "sqlite://" + str(restored / "database/gateway.sqlite"))
    text = text.replace(str(resources.directory / "spool"), str(restored / "spool"))
    config.write_text(text)
    resources.database_file = restored / "database/gateway.sqlite"
    start_gateway(resources, binary, data, "gateway-restored")
    verify_settlement(data, count, warmups)
    for name in unknown:
        check((restored / "spool/admissions" / name).is_file(), "restore lost uncertain usage")
    return [
        {"id": "sqlite-single-owner-cli", "status": "passed"},
        {"id": "sqlite-offline-paired-backup-restore", "status": "passed",
         "uncertain_usage_retained": True, "settlement_unchanged": True},
        {"id": "sqlite-restore-rejects-existing-and-corrupt", "status": "passed"},
        {"id": "sqlite-restore-enospc-does-not-publish", "status": "passed"},
    ]
