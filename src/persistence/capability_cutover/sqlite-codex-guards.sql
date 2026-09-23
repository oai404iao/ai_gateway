-- Install after backfill so existing uncertain operations survive migration.
CREATE TRIGGER canonical_access_codex_operation_fence BEFORE UPDATE ON upstream_accesses
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.access_id=OLD.id AND c.deleted_at IS NULL
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_channel_codex_operation_insert_fence BEFORE INSERT ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id=NEW.credential_id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_channel_codex_operation_update_fence BEFORE UPDATE ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id IN (OLD.credential_id,NEW.credential_id)
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_capability_codex_operation_insert_fence BEFORE INSERT ON channel_capabilities
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.id=NEW.channel_id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_capability_codex_operation_update_fence BEFORE UPDATE ON channel_capabilities
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.id IN (OLD.channel_id,NEW.channel_id)
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_credential_codex_operation_fence BEFORE UPDATE ON upstream_credentials
WHEN OLD.kind='codex_oauth' AND EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;
