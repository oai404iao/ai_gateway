-- Retain ordinary channel and channel-group tombstones while releasing their
-- natural names and preserving request-log foreign keys.
ALTER TABLE channel_groups
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT,
    ADD CONSTRAINT channel_groups_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL)),
    ADD CONSTRAINT channel_groups_deleted_state_check
        CHECK (
            deleted_at IS NULL
            OR (
                connector_kind = 'openai_compatible'
                AND NOT enabled
                AND NOT status_statistics_enabled
            )
        );

ALTER TABLE channels
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT,
    ADD CONSTRAINT channels_deleted_actor_check
        CHECK ((deleted_at IS NULL) = (deleted_by IS NULL)),
    ADD CONSTRAINT channels_deleted_state_check
        CHECK (
            deleted_at IS NULL
            OR (
                NOT enabled
                AND NOT auto_disabled
                AND NOT auto_disable_allowed
                AND NOT supports_websocket
                AND NOT supports_standalone_web_search
                AND base_url = 'https://deleted.invalid'
                AND billing_multiplier = 1
                AND proxy_id IS NULL
                AND config_template_id IS NULL
                AND override_document = '{}'::jsonb
                AND connect_timeout_ms IS NULL
                AND response_header_timeout_ms IS NULL
                AND stream_idle_timeout_ms IS NULL
                AND upstream_auth_kind = 'none'
                AND upstream_auth_header_name IS NULL
                AND upstream_api_key IS NULL
                AND cardinality(available_models) = 0
                AND test_model IS NULL
                AND test_pricing_model_id IS NULL
            )
        );

ALTER TABLE channel_groups
    DROP CONSTRAINT channel_groups_name_key;

CREATE UNIQUE INDEX channel_groups_active_name_idx
    ON channel_groups (name)
    WHERE deleted_at IS NULL;

ALTER TABLE channels
    DROP CONSTRAINT channels_channel_group_id_name_key;

CREATE UNIQUE INDEX channels_active_group_name_idx
    ON channels (channel_group_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX channel_groups_deleted_at_idx
    ON channel_groups (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX channels_deleted_at_idx
    ON channels (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE FUNCTION enforce_channel_tombstone_lifecycle()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    owning_connector_kind text;
    owning_group_deleted_at timestamptz;
BEGIN
    IF TG_TABLE_NAME = 'channel_groups' THEN
        IF OLD.deleted_at IS NOT NULL
           AND (
               NEW.deleted_at IS DISTINCT FROM OLD.deleted_at
               OR NEW.deleted_by IS DISTINCT FROM OLD.deleted_by
           )
        THEN
            RAISE EXCEPTION USING
                ERRCODE = 'check_violation',
                MESSAGE = 'channel-group tombstones cannot be restored or reassigned';
        END IF;

        IF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN
            IF NEW.connector_kind <> 'openai_compatible' THEN
                RAISE EXCEPTION USING
                    ERRCODE = 'check_violation',
                    MESSAGE = 'provider-managed channel groups use their connector lifecycle';
            END IF;
            IF EXISTS (
                SELECT 1
                FROM channels
                WHERE channel_group_id = NEW.id
                  AND deleted_at IS NULL
            ) THEN
                RAISE EXCEPTION USING
                    ERRCODE = 'check_violation',
                    MESSAGE = 'a channel group cannot be tombstoned before its channels';
            END IF;
        END IF;
        RETURN NEW;
    END IF;

    IF TG_OP = 'UPDATE'
       AND OLD.deleted_at IS NOT NULL
       AND (
           NEW.deleted_at IS DISTINCT FROM OLD.deleted_at
           OR NEW.deleted_by IS DISTINCT FROM OLD.deleted_by
       )
    THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'channel tombstones cannot be restored or reassigned';
    END IF;

    SELECT connector_kind, deleted_at
    INTO owning_connector_kind, owning_group_deleted_at
    FROM channel_groups
    WHERE id = NEW.channel_group_id;

    IF NEW.deleted_at IS NOT NULL
       AND owning_connector_kind <> 'openai_compatible'
    THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'provider-managed channels use their connector lifecycle';
    END IF;

    IF NEW.deleted_at IS NULL AND owning_group_deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = 'active channels cannot belong to a deleted channel group';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channel_groups_enforce_tombstone_lifecycle
BEFORE UPDATE OF deleted_at, deleted_by ON channel_groups
FOR EACH ROW EXECUTE FUNCTION enforce_channel_tombstone_lifecycle();

CREATE TRIGGER channels_enforce_tombstone_lifecycle
BEFORE INSERT OR UPDATE OF channel_group_id, deleted_at, deleted_by ON channels
FOR EACH ROW EXECUTE FUNCTION enforce_channel_tombstone_lifecycle();

CREATE FUNCTION prevent_channel_tombstone_update()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.deleted_at IS NOT NULL THEN
        RAISE EXCEPTION USING
            ERRCODE = 'check_violation',
            MESSAGE = format('%s tombstones are immutable', TG_TABLE_NAME);
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channel_groups_prevent_tombstone_update
AFTER UPDATE ON channel_groups
FOR EACH ROW EXECUTE FUNCTION prevent_channel_tombstone_update();

CREATE TRIGGER channels_prevent_tombstone_update
AFTER UPDATE ON channels
FOR EACH ROW EXECUTE FUNCTION prevent_channel_tombstone_update();

CREATE FUNCTION prevent_channel_tombstone_hard_delete()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION USING
        ERRCODE = 'check_violation',
        MESSAGE = format('%s rows must be retained as soft-delete tombstones', TG_TABLE_NAME);
END;
$$;

CREATE TRIGGER channel_groups_prevent_hard_delete
BEFORE DELETE ON channel_groups
FOR EACH ROW EXECUTE FUNCTION prevent_channel_tombstone_hard_delete();

CREATE TRIGGER channels_prevent_hard_delete
BEFORE DELETE ON channels
FOR EACH ROW EXECUTE FUNCTION prevent_channel_tombstone_hard_delete();
