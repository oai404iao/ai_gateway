-- The startup writer first removes the obsolete pool/group columns with a
-- checked table rebuild. The remaining changes share that transaction.
-- Backfill metadata without clearing uncertain provider-operation intents.
DROP TRIGGER codex_operation_update_fence;
DROP TRIGGER canonical_channel_codex_operation_update_fence;
ALTER TABLE upstream_credentials ADD COLUMN connector_kind TEXT
    GENERATED ALWAYS AS (CASE WHEN kind='codex_oauth' THEN 'codex' ELSE 'general' END) VIRTUAL;
ALTER TABLE upstream_channels ADD COLUMN sharing_only INTEGER NOT NULL DEFAULT 0
    CHECK (sharing_only IN (0,1));
UPDATE upstream_channels SET sharing_only=1,updated_at=ag_now()
WHERE group_id IN (SELECT id FROM routing_groups WHERE sharing_only=1) AND deleted_at IS NULL;

ALTER TABLE codex_oauth_credentials ADD COLUMN proxy_id TEXT
    REFERENCES proxies(id) ON DELETE RESTRICT CHECK(proxy_id IS NULL OR ag_uuid_valid(proxy_id));
ALTER TABLE codex_oauth_credentials ADD COLUMN available_models TEXT NOT NULL DEFAULT '[]'
    CHECK(json_valid(available_models) AND json_type(available_models)='array');
UPDATE codex_oauth_credentials SET
    proxy_id=(SELECT a.proxy_id FROM upstream_channels ch
        JOIN upstream_accesses a ON a.id=ch.access_id
        WHERE ch.id=codex_oauth_credentials.channel_id
          AND ch.credential_id=codex_oauth_credentials.channel_id),
    available_models=COALESCE((SELECT cap.available_models FROM channel_capabilities cap
        WHERE cap.channel_id=codex_oauth_credentials.channel_id AND cap.operation='responses'
        ORDER BY cap.deleted_at NULLS FIRST LIMIT 1),'[]'),
    updated_at=ag_now();

ALTER TABLE codex_sharing_groups ADD COLUMN channel_id TEXT
    REFERENCES upstream_channels(id) ON DELETE RESTRICT
    CHECK(channel_id IS NULL OR ag_uuid_valid(channel_id));
UPDATE codex_sharing_groups SET channel_id=(
    SELECT c.id FROM upstream_channels c WHERE c.id=codex_sharing_groups.credential_id
      AND c.credential_id=codex_sharing_groups.credential_id
),updated_at=ag_now();
CREATE TEMP TABLE _sharing_channel_upgrade_check(valid INTEGER CHECK(valid=1));
INSERT INTO _sharing_channel_upgrade_check
SELECT NOT EXISTS(SELECT 1 FROM codex_sharing_groups WHERE channel_id IS NULL);
DROP TABLE _sharing_channel_upgrade_check;
CREATE UNIQUE INDEX codex_sharing_channel_unique ON codex_sharing_groups(channel_id);
UPDATE codex_oauth_flows SET completed_at=ag_now() WHERE completed_at IS NULL;
DROP TABLE connector_pools;
ALTER TABLE routing_groups DROP COLUMN sharing_only;
CREATE INDEX codex_credentials_account_identity_idx
    ON codex_oauth_credentials(account_id,user_id) WHERE deleted_at IS NULL;

DROP TRIGGER codex_sharing_binding_guard;
CREATE TRIGGER codex_sharing_binding_guard BEFORE UPDATE ON codex_sharing_groups
WHEN NEW.channel_id IS NULL OR NEW.id<>OLD.id OR NEW.channel_id<>OLD.channel_id OR NEW.credential_id<>OLD.credential_id
 OR NEW.provider_account_id<>OLD.provider_account_id OR NEW.provider_user_id<>OLD.provider_user_id
 OR json_array_length(NEW.seats)<json_array_length(OLD.seats)
BEGIN SELECT RAISE(ABORT,'sharing_binding_immutable'); END;
CREATE TRIGGER codex_sharing_channel_required BEFORE INSERT ON codex_sharing_groups
WHEN NOT EXISTS (
    SELECT 1 FROM upstream_channels ch
    JOIN upstream_accesses a ON a.id=ch.access_id
    JOIN codex_oauth_credentials c ON c.channel_id=ch.credential_id
    WHERE ch.id=NEW.channel_id AND ch.deleted_at IS NULL
      AND ch.credential_id=NEW.credential_id AND c.deleted_at IS NULL
      AND a.connector_kind='codex'
      AND COALESCE(c.account_id,'')=NEW.provider_account_id AND c.user_id=NEW.provider_user_id
)
BEGIN SELECT RAISE(ABORT,'sharing_channel_identity'); END;
CREATE TRIGGER codex_sharing_channel_update BEFORE UPDATE ON codex_sharing_groups
WHEN NOT EXISTS (
    SELECT 1 FROM upstream_channels ch
    JOIN upstream_accesses a ON a.id=ch.access_id
    JOIN codex_oauth_credentials c ON c.channel_id=ch.credential_id
    WHERE ch.id=NEW.channel_id AND ch.deleted_at IS NULL
      AND ch.credential_id=NEW.credential_id AND c.deleted_at IS NULL
      AND a.connector_kind='codex'
      AND COALESCE(c.account_id,'')=NEW.provider_account_id AND c.user_id=NEW.provider_user_id
)
BEGIN SELECT RAISE(ABORT,'sharing_channel_identity'); END;

CREATE TRIGGER upstream_channel_sharing_insert BEFORE INSERT ON upstream_channels
WHEN NEW.sharing_only AND NOT EXISTS (
    SELECT 1 FROM upstream_credentials c JOIN upstream_accesses a ON a.id=NEW.access_id
    WHERE c.id=NEW.credential_id AND c.connector_kind='codex' AND a.connector_kind='codex'
)
BEGIN SELECT RAISE(ABORT,'sharing_connector_required'); END;
CREATE TRIGGER upstream_channel_sharing_update BEFORE UPDATE ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM codex_sharing_groups s WHERE s.channel_id=NEW.id
      AND (NEW.credential_id IS NOT s.credential_id OR NEW.deleted_at IS NOT NULL)
) OR (NEW.sharing_only AND NOT EXISTS (
    SELECT 1 FROM upstream_credentials c JOIN upstream_accesses a ON a.id=NEW.access_id
    WHERE c.id=NEW.credential_id AND c.connector_kind='codex' AND a.connector_kind='codex'
))
BEGIN SELECT RAISE(ABORT,'sharing_channel_binding'); END;

DROP TRIGGER upstream_credentials_lifecycle;
CREATE TRIGGER upstream_credentials_lifecycle BEFORE UPDATE ON upstream_credentials
WHEN OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind
 OR (NEW.deleted_at IS NOT NULL AND EXISTS (
    SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
))
BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;

CREATE TRIGGER codex_operation_update_fence BEFORE UPDATE ON codex_oauth_credentials
WHEN EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.channel_id)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;
CREATE TRIGGER canonical_channel_codex_operation_update_fence BEFORE UPDATE ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id IN (OLD.credential_id,NEW.credential_id)
)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;
