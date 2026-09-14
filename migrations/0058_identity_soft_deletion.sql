-- Retain identity control-plane tombstones while allowing natural names to be
-- reused. Request-log and audit foreign keys continue to address the original
-- UUIDs.
ALTER TABLE api_keys
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT,
    ADD CONSTRAINT api_keys_deleted_state_check
        CHECK (deleted_at IS NULL OR status = 'revoked'),
    ADD CONSTRAINT api_keys_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL));

ALTER TABLE user_groups
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT,
    ADD CONSTRAINT user_groups_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL));

ALTER TABLE users
    ADD CONSTRAINT users_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL));

ALTER TABLE api_keys
    DROP CONSTRAINT api_keys_user_id_name_key;

CREATE UNIQUE INDEX api_keys_active_user_name_idx
    ON api_keys (user_id, name)
    WHERE deleted_at IS NULL;

ALTER TABLE user_groups
    DROP CONSTRAINT user_groups_name_key;

CREATE UNIQUE INDEX user_groups_active_name_idx
    ON user_groups (name)
    WHERE deleted_at IS NULL;

CREATE INDEX api_keys_deleted_at_idx
    ON api_keys (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX user_groups_deleted_at_idx
    ON user_groups (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE FUNCTION prevent_identity_tombstone_hard_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = format('%s rows must be retained as soft-delete tombstones', TG_TABLE_NAME);
END;
$$;

CREATE TRIGGER users_prevent_hard_delete
BEFORE DELETE ON users
FOR EACH ROW EXECUTE FUNCTION prevent_identity_tombstone_hard_delete();

CREATE TRIGGER user_groups_prevent_hard_delete
BEFORE DELETE ON user_groups
FOR EACH ROW EXECUTE FUNCTION prevent_identity_tombstone_hard_delete();

CREATE TRIGGER api_keys_prevent_hard_delete
BEFORE DELETE ON api_keys
FOR EACH ROW EXECUTE FUNCTION prevent_identity_tombstone_hard_delete();
