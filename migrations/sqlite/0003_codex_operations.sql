CREATE TABLE _gateway_codex_operations (
    credential_id TEXT PRIMARY KEY REFERENCES codex_oauth_credentials(channel_id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE CHECK(ag_uuid_valid(attempt_id)),
    kind TEXT NOT NULL CHECK(kind IN ('refresh','quota_reset')),
    generation INTEGER NOT NULL CHECK(generation>=0),
    started_at TEXT NOT NULL DEFAULT (ag_now()) CHECK(ag_time_valid(started_at))
) STRICT;

CREATE TRIGGER codex_operation_update_fence BEFORE UPDATE ON codex_oauth_credentials
WHEN EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.channel_id)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

CREATE TRIGGER codex_operation_delete_fence BEFORE DELETE ON codex_oauth_credentials
WHEN EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.channel_id)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

CREATE TRIGGER codex_operation_channel_fence BEFORE UPDATE ON channels
WHEN EXISTS(
    SELECT 1 FROM _gateway_codex_operations o
    JOIN codex_oauth_credential_channels p ON p.credential_id=o.credential_id
    WHERE p.channel_id=OLD.id
)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;
