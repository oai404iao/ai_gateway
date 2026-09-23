DROP TRIGGER codex_identity_insert;
DROP TRIGGER codex_identity_update;
DROP TRIGGER codex_projection_identity;
DROP TRIGGER channels_codex_identity_name;
DROP TRIGGER codex_create_projections;
DROP TRIGGER codex_sync_images;
DROP TRIGGER codex_tombstone_images;
DROP TRIGGER codex_operation_channel_fence;
DROP TRIGGER upstream_credentials_lifecycle;
DROP TRIGGER credential_pool_insert;
DROP TRIGGER credential_pool_update;

CREATE TRIGGER credential_pool_insert BEFORE INSERT ON codex_oauth_credentials
WHEN NOT EXISTS (SELECT 1 FROM connector_pools
                 WHERE id=NEW.connector_pool_id AND routing_group_id=NEW.channel_group_id)
BEGIN SELECT RAISE(ABORT,'codex_credential_pool_mismatch'); END;
CREATE TRIGGER credential_pool_update BEFORE UPDATE OF channel_group_id,connector_pool_id ON codex_oauth_credentials
WHEN NOT EXISTS (SELECT 1 FROM connector_pools
                 WHERE id=NEW.connector_pool_id AND routing_group_id=NEW.channel_group_id)
BEGIN SELECT RAISE(ABORT,'codex_credential_pool_mismatch'); END;

UPDATE codex_oauth_credentials SET
    channel_group_id=COALESCE((SELECT canonical_group_id FROM group_identity_registry
                              WHERE id=codex_oauth_credentials.channel_group_id),channel_group_id),
    updated_at=ag_now()
WHERE channel_group_id IS NOT (SELECT canonical_group_id FROM group_identity_registry
                              WHERE id=codex_oauth_credentials.channel_group_id)
  AND EXISTS (SELECT 1 FROM group_identity_registry
              WHERE id=codex_oauth_credentials.channel_group_id AND canonical_group_id IS NOT NULL);
UPDATE codex_oauth_flows SET
    channel_group_id=COALESCE((SELECT canonical_group_id FROM group_identity_registry
                              WHERE id=codex_oauth_flows.channel_group_id),channel_group_id);
INSERT INTO user_group_codex_quota_visibility(user_group_id,channel_group_id,created_at)
SELECT v.user_group_id,r.canonical_group_id,v.created_at
FROM user_group_codex_quota_visibility v JOIN group_identity_registry r ON r.id=v.channel_group_id
WHERE r.canonical_group_id IS NOT NULL AND r.id<>r.canonical_group_id
ON CONFLICT DO NOTHING;
DELETE FROM user_group_codex_quota_visibility
WHERE channel_group_id IN (SELECT id FROM group_identity_registry
                          WHERE canonical_group_id IS NOT NULL AND id<>canonical_group_id);

CREATE TRIGGER upstream_credentials_lifecycle BEFORE UPDATE ON upstream_credentials
WHEN OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind OR (
    NEW.kind<>'codex_oauth' AND NEW.deleted_at IS NOT NULL AND EXISTS (
        SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
    )
) BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;

CREATE TRIGGER codex_token_revision AFTER UPDATE OF access_token ON codex_oauth_credentials
WHEN OLD.access_token IS NOT NEW.access_token AND NEW.deleted_at IS NULL
BEGIN
    UPDATE upstream_credentials SET revision=ag_md5_uuid(hex(randomblob(32))),updated_at=ag_now()
    WHERE id=NEW.channel_id;
END;
