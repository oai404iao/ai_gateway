-- Quarantine every formerly protected identity before removing its admission policy.
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
UPDATE upstream_channels SET enabled = false
WHERE deleted_at IS NULL AND (
    sharing_only OR credential_id IN (SELECT credential_id FROM protected_credentials)
    OR id IN (SELECT channel_id FROM codex_sharing_groups)
);

DROP TRIGGER upstream_channel_sharing_guard ON upstream_channels;
DROP TRIGGER codex_sharing_identity_guard ON codex_oauth_credentials;
DROP TABLE codex_sharing_groups;
DROP TABLE codex_sharing_ledger;
DROP FUNCTION validate_channel_sharing_binding();
DROP FUNCTION validate_codex_sharing_channel();
DROP FUNCTION protect_codex_sharing_binding();
DROP FUNCTION protect_codex_sharing_identity();
ALTER TABLE upstream_channels DROP COLUMN sharing_only;
