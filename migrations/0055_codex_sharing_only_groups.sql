-- Routing policy only: existing sharing bindings, windows and ledger identity
-- remain untouched. Bound credentials retain protection regardless of this flag.
ALTER TABLE channel_groups
    ADD COLUMN sharing_only boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT channel_groups_sharing_only_codex_check
        CHECK (NOT sharing_only OR connector_kind = 'codex_oauth');

-- Access mode belongs to the logical Codex pool. Keep its Responses/Images
-- controls consistent without enabling either format or granting API Key access.
CREATE FUNCTION sync_codex_sharing_only_groups()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.connector_kind <> 'codex_oauth' THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'INSERT' THEN
        NEW.sharing_only := COALESCE(
            (SELECT sharing_only FROM channel_groups
             WHERE connector_pool_id=NEW.connector_pool_id AND id<>NEW.id LIMIT 1),
            NEW.sharing_only);
    ELSE
        UPDATE channel_groups SET sharing_only=NEW.sharing_only
        WHERE connector_pool_id=NEW.connector_pool_id AND id<>NEW.id
          AND sharing_only IS DISTINCT FROM NEW.sharing_only;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER channel_groups_sharing_only_insert
BEFORE INSERT ON channel_groups
FOR EACH ROW EXECUTE FUNCTION sync_codex_sharing_only_groups();
CREATE TRIGGER channel_groups_sharing_only_update
AFTER UPDATE OF sharing_only ON channel_groups
FOR EACH ROW WHEN (OLD.sharing_only IS DISTINCT FROM NEW.sharing_only)
EXECUTE FUNCTION sync_codex_sharing_only_groups();
