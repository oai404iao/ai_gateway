-- Quarantine does not dispatch provider operations or resolve uncertain intents.
DROP TRIGGER canonical_channel_codex_operation_update_fence;
WITH protected AS (
    SELECT credential_id, provider_account_id, provider_user_id FROM codex_sharing_groups
    UNION
    SELECT c.credential_id, COALESCE(identity.account_id, ''), identity.user_id
    FROM upstream_channels c
    JOIN codex_oauth_credentials identity ON identity.channel_id = c.credential_id
    WHERE c.sharing_only AND c.deleted_at IS NULL
), protected_credentials AS (
    SELECT credential_id FROM protected
    UNION
    SELECT candidate.channel_id FROM codex_oauth_credentials candidate
    JOIN protected source ON COALESCE(candidate.account_id, '') = source.provider_account_id
        AND candidate.user_id = source.provider_user_id
)
UPDATE upstream_channels SET enabled = 0, updated_at = ag_now()
WHERE deleted_at IS NULL AND (
    sharing_only OR credential_id IN (SELECT credential_id FROM protected_credentials)
    OR id IN (SELECT channel_id FROM codex_sharing_groups)
);
CREATE TRIGGER canonical_channel_codex_operation_update_fence BEFORE UPDATE ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id IN (OLD.credential_id,NEW.credential_id)
)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

DROP TRIGGER upstream_channel_sharing_insert;
DROP TRIGGER upstream_channel_sharing_update;
DROP TRIGGER codex_sharing_identity_guard;
DROP TABLE codex_sharing_groups;
DROP TABLE codex_sharing_ledger;
ALTER TABLE upstream_channels DROP COLUMN sharing_only;
