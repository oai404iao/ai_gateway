DROP TRIGGER codex_oauth_credentials_identity ON codex_oauth_credentials;
DROP TRIGGER codex_oauth_credential_channels_identity ON codex_oauth_credential_channels;
DROP TRIGGER channels_codex_identity_name ON channels;
DROP TRIGGER codex_oauth_credentials_create_projections ON codex_oauth_credentials;
DROP TRIGGER channels_sync_codex_images_projection ON channels;
DROP TRIGGER codex_oauth_credentials_tombstone_images_projection ON codex_oauth_credentials;

ALTER TABLE codex_oauth_credentials
    DROP CONSTRAINT codex_oauth_credentials_channel_id_fkey,
    ADD CONSTRAINT codex_oauth_credentials_channel_id_fkey
        FOREIGN KEY (channel_id) REFERENCES upstream_credentials(id) ON DELETE RESTRICT;

CREATE OR REPLACE FUNCTION guard_upstream_credential_lifecycle()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION USING ERRCODE = 'check_violation', MESSAGE = 'upstream_credentials_lifecycle';
    END IF;
    IF OLD.deleted_at IS NOT NULL OR NEW.id <> OLD.id OR NEW.kind <> OLD.kind OR
       (NEW.kind <> 'codex_oauth' AND NEW.deleted_at IS NOT NULL AND EXISTS (
            SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
       )) THEN
        RAISE EXCEPTION USING ERRCODE = 'check_violation', MESSAGE = 'upstream_credentials_lifecycle';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION invalidate_codex_credential_token()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.access_token IS DISTINCT FROM NEW.access_token AND NEW.deleted_at IS NULL THEN
        UPDATE upstream_credentials SET revision=gen_random_uuid() WHERE id=NEW.channel_id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER codex_token_revision AFTER UPDATE OF access_token ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION invalidate_codex_credential_token();
