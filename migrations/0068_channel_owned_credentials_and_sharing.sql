-- Preserve identities and money while removing the old Codex pool ownership.
ALTER TABLE upstream_credentials
    ADD COLUMN connector_kind text GENERATED ALWAYS AS (
        CASE WHEN kind='codex_oauth' THEN 'codex' ELSE 'general' END
    ) STORED;
ALTER TABLE upstream_credentials DROP CONSTRAINT upstream_credentials_auth_check;
UPDATE upstream_credentials credential SET allowed_base_urls=(
    SELECT COALESCE(jsonb_agg(DISTINCT a.base_url),'[]'::jsonb)
    FROM upstream_channels c JOIN upstream_accesses a ON a.id=c.access_id
    WHERE c.credential_id=credential.id
) WHERE kind='codex_oauth' AND deleted_at IS NULL;
ALTER TABLE upstream_credentials ADD CONSTRAINT upstream_credentials_auth_check CHECK (
    (deleted_at IS NOT NULL AND secret IS NULL AND NOT enabled)
    OR (deleted_at IS NULL AND jsonb_array_length(allowed_base_urls)>0 AND (
        (kind='codex_oauth' AND secret IS NULL AND header_name IS NULL)
        OR (kind='bearer' AND secret IS NOT NULL AND length(btrim(secret))>0 AND header_name IS NULL)
        OR (kind='header' AND secret IS NOT NULL AND length(btrim(secret))>0 AND header_name IS NOT NULL)
    ))
);

ALTER TABLE upstream_channels ADD COLUMN sharing_only boolean NOT NULL DEFAULT false;
UPDATE upstream_channels c SET sharing_only=g.sharing_only
FROM routing_groups g WHERE g.id=c.group_id AND g.sharing_only AND c.deleted_at IS NULL;

ALTER TABLE codex_oauth_credentials
    ADD COLUMN proxy_id uuid REFERENCES proxies(id) ON DELETE RESTRICT,
    ADD COLUMN available_models jsonb NOT NULL DEFAULT '[]'
        CHECK (jsonb_typeof(available_models)='array');
UPDATE codex_oauth_credentials c SET proxy_id=a.proxy_id,
    available_models=COALESCE((
        SELECT to_jsonb(cap.available_models) FROM channel_capabilities cap
        WHERE cap.channel_id=ch.id AND cap.operation='responses'
        ORDER BY cap.deleted_at NULLS FIRST LIMIT 1
    ),'[]'::jsonb)
FROM upstream_channels ch JOIN upstream_accesses a ON a.id=ch.access_id
WHERE ch.credential_id=c.channel_id AND ch.id=c.channel_id;

ALTER TABLE codex_sharing_groups ADD COLUMN channel_id uuid
    REFERENCES upstream_channels(id) ON DELETE RESTRICT;
UPDATE codex_sharing_groups s SET channel_id=c.id
FROM upstream_channels c
WHERE c.id=s.credential_id AND c.credential_id=s.credential_id;
ALTER TABLE codex_sharing_groups ALTER COLUMN channel_id SET NOT NULL;
ALTER TABLE codex_sharing_groups ADD CONSTRAINT codex_sharing_channel_unique UNIQUE(channel_id);

DROP TRIGGER codex_oauth_credentials_set_connector_pool ON codex_oauth_credentials;
DROP FUNCTION set_codex_credential_connector_pool();
DROP INDEX codex_oauth_credentials_pool_identity_idx;
ALTER TABLE codex_oauth_credentials
    DROP COLUMN channel_group_id,
    DROP COLUMN connector_pool_id;

-- Old pending flows selected a group and implicitly created routing objects.
UPDATE codex_oauth_flows SET completed_at=now() WHERE completed_at IS NULL;
ALTER TABLE codex_oauth_flows DROP COLUMN channel_group_id;
DROP TABLE connector_pools;
ALTER TABLE routing_groups DROP COLUMN sharing_only;

-- Keep duplicate historical imports distinct; future imports resolve one
-- unambiguous identity or fail rather than silently merging existing records.
CREATE INDEX codex_credentials_account_identity_idx
    ON codex_oauth_credentials(account_id,user_id) WHERE deleted_at IS NULL;

CREATE OR REPLACE FUNCTION protect_codex_sharing_binding()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id<>OLD.id OR NEW.channel_id<>OLD.channel_id
       OR NEW.credential_id<>OLD.credential_id
       OR NEW.provider_account_id<>OLD.provider_account_id
       OR NEW.provider_user_id<>OLD.provider_user_id
       OR jsonb_array_length(NEW.seats)<jsonb_array_length(OLD.seats) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation',
            MESSAGE='sharing bindings and existing seat numbers are immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE FUNCTION validate_codex_sharing_channel()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM upstream_channels ch
        JOIN upstream_accesses a ON a.id=ch.access_id
        JOIN codex_oauth_credentials c ON c.channel_id=ch.credential_id
        WHERE ch.id=NEW.channel_id AND ch.deleted_at IS NULL
          AND ch.credential_id=NEW.credential_id AND c.deleted_at IS NULL
          AND a.connector_kind='codex'
          AND COALESCE(c.account_id,'')=NEW.provider_account_id AND c.user_id=NEW.provider_user_id
    ) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='sharing_channel_identity';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER codex_sharing_channel_guard BEFORE INSERT OR UPDATE ON codex_sharing_groups
FOR EACH ROW EXECUTE FUNCTION validate_codex_sharing_channel();

CREATE FUNCTION validate_channel_sharing_binding()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM codex_sharing_groups s WHERE s.channel_id=NEW.id
          AND (NEW.credential_id IS DISTINCT FROM s.credential_id OR NEW.deleted_at IS NOT NULL)
    ) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation',
            MESSAGE='a sharing channel binding cannot be replaced or deleted';
    END IF;
    IF NEW.sharing_only AND NOT EXISTS (
        SELECT 1 FROM upstream_credentials c
        JOIN upstream_accesses a ON a.id=NEW.access_id
        WHERE c.id=NEW.credential_id AND c.connector_kind='codex'
          AND a.connector_kind='codex'
    ) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation',
            MESSAGE='sharing requires a supported credential and access';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER upstream_channel_sharing_guard BEFORE INSERT OR UPDATE ON upstream_channels
FOR EACH ROW EXECUTE FUNCTION validate_channel_sharing_binding();

CREATE OR REPLACE FUNCTION guard_upstream_credential_lifecycle()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='upstream_credentials_lifecycle';
    END IF;
    IF OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind OR
       (NEW.deleted_at IS NOT NULL AND EXISTS (
            SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
       )) THEN
        RAISE EXCEPTION USING ERRCODE='check_violation', MESSAGE='upstream_credentials_lifecycle';
    END IF;
    RETURN NEW;
END;
$$;
