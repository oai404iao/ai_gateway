DROP TRIGGER codex_identity_insert;
DROP TRIGGER codex_identity_update;
DROP TRIGGER codex_projection_identity;
DROP TRIGGER channels_codex_identity_name;
DROP TRIGGER codex_create_projections;
DROP TRIGGER codex_sync_images;
DROP TRIGGER codex_tombstone_images;
DROP TRIGGER codex_operation_channel_fence;
DROP TRIGGER upstream_credentials_lifecycle;

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
