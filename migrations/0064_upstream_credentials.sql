CREATE TABLE upstream_credentials (
    id uuid PRIMARY KEY,
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    kind text NOT NULL CHECK (kind IN ('bearer','header','codex_oauth')),
    header_name text,
    secret text,
    allowed_base_urls jsonb NOT NULL DEFAULT '[]',
    enabled boolean NOT NULL DEFAULT true,
    revision uuid NOT NULL DEFAULT gen_random_uuid(),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    CONSTRAINT upstream_credentials_scope_check CHECK (jsonb_typeof(allowed_base_urls)='array'),
    CONSTRAINT upstream_credentials_auth_check CHECK (
        (deleted_at IS NOT NULL AND secret IS NULL AND NOT enabled)
        OR (deleted_at IS NULL AND (
            (kind='codex_oauth' AND secret IS NULL AND header_name IS NULL AND allowed_base_urls='[]')
            OR (kind='bearer' AND secret IS NOT NULL AND length(btrim(secret))>0
                AND header_name IS NULL AND jsonb_array_length(allowed_base_urls)>0)
            OR (kind='header' AND secret IS NOT NULL AND length(btrim(secret))>0
                AND header_name IS NOT NULL AND jsonb_array_length(allowed_base_urls)>0)
        ))
    )
);
CREATE TRIGGER upstream_credentials_updated_at BEFORE UPDATE ON upstream_credentials
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

ALTER TABLE channels ADD COLUMN credential_id uuid
    REFERENCES upstream_credentials(id) ON DELETE RESTRICT;
ALTER TABLE channels ADD COLUMN credential_binding_revision uuid NOT NULL DEFAULT '00000000-0000-0000-0000-000000000000';
CREATE INDEX channels_credential_id_idx ON channels(credential_id) WHERE credential_id IS NOT NULL;

INSERT INTO upstream_credentials(id,name,kind,header_name,secret,allowed_base_urls,created_at,updated_at)
SELECT id,name,upstream_auth_kind,upstream_auth_header_name,upstream_api_key,
       jsonb_build_array(base_url),created_at,updated_at
FROM channels WHERE deleted_at IS NULL AND upstream_auth_kind<>'none';
INSERT INTO upstream_credentials(id,name,kind,enabled,created_at,updated_at,deleted_at)
SELECT co.channel_id,c.name,'codex_oauth',co.enabled AND co.deleted_at IS NULL,
       co.created_at,co.updated_at,co.deleted_at
FROM codex_oauth_credentials co JOIN channels c ON c.id=co.channel_id;
UPDATE channels SET credential_id=id,
    upstream_auth_kind='none',upstream_auth_header_name=NULL,upstream_api_key=NULL
WHERE deleted_at IS NULL AND upstream_auth_kind<>'none';
UPDATE channels c SET credential_id=p.credential_id
FROM codex_oauth_credential_channels p WHERE p.channel_id=c.id AND c.deleted_at IS NULL;

-- These empty compatibility columns let historical projection functions remain unchanged.
ALTER TABLE channels ADD CONSTRAINT channels_auth_externalized_check
CHECK (upstream_auth_kind='none' AND upstream_auth_header_name IS NULL AND upstream_api_key IS NULL);

CREATE FUNCTION sync_codex_upstream_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='INSERT' THEN
        INSERT INTO upstream_credentials(id,name,kind,enabled,created_at,updated_at,deleted_at)
        SELECT NEW.channel_id,c.name,'codex_oauth',NEW.enabled AND NEW.deleted_at IS NULL,
               NEW.created_at,NEW.updated_at,NEW.deleted_at
        FROM channels c WHERE c.id=NEW.channel_id;
    ELSE
        UPDATE upstream_credentials SET enabled=NEW.enabled AND NEW.deleted_at IS NULL,
            deleted_at=NEW.deleted_at,
            revision=CASE WHEN OLD.enabled IS DISTINCT FROM NEW.enabled
                OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at
                OR OLD.access_token IS DISTINCT FROM NEW.access_token
                THEN gen_random_uuid() ELSE revision END
        WHERE id=NEW.channel_id;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER codex_oauth_credentials_identity BEFORE INSERT OR UPDATE ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION sync_codex_upstream_identity();

CREATE FUNCTION bind_codex_upstream_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE channels SET credential_id=NEW.credential_id WHERE id=NEW.channel_id;
    RETURN NEW;
END;
$$;
CREATE TRIGGER codex_oauth_credential_channels_identity AFTER INSERT ON codex_oauth_credential_channels
FOR EACH ROW EXECUTE FUNCTION bind_codex_upstream_identity();

CREATE FUNCTION guard_upstream_credential_binding() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE credential upstream_credentials; connector text;
BEGIN
    IF TG_OP='UPDATE' AND OLD.credential_id IS DISTINCT FROM NEW.credential_id THEN
        NEW.credential_binding_revision := gen_random_uuid();
    END IF;
    IF NEW.credential_id IS NULL THEN
        IF EXISTS (SELECT 1 FROM codex_oauth_credential_channels WHERE channel_id=NEW.id) THEN
            RAISE EXCEPTION 'managed credential binding is immutable' USING ERRCODE='23514';
        END IF;
        RETURN NEW;
    END IF;
    SELECT * INTO credential FROM upstream_credentials WHERE id=NEW.credential_id FOR SHARE;
    SELECT connector_kind INTO connector FROM channel_groups WHERE id=NEW.channel_group_id;
    IF credential.id IS NULL OR
       (credential.deleted_at IS NOT NULL AND credential.kind<>'codex_oauth') OR
       (credential.kind='codex_oauth') <> (connector='codex_oauth') OR
       (credential.kind='codex_oauth' AND NOT EXISTS (
           SELECT 1 FROM codex_oauth_credential_channels
           WHERE channel_id=NEW.id AND credential_id=NEW.credential_id))
    THEN
        RAISE EXCEPTION 'invalid upstream credential binding' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER channels_credential_binding BEFORE INSERT OR UPDATE OF credential_id,channel_group_id ON channels
FOR EACH ROW EXECUTE FUNCTION guard_upstream_credential_binding();

CREATE FUNCTION guard_upstream_credential_lifecycle() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'upstream credential identities require soft deletion' USING ERRCODE='23514';
    END IF;
    IF OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind THEN
        RAISE EXCEPTION 'upstream credential identity is immutable' USING ERRCODE='23514';
    END IF;
    IF NEW.kind<>'codex_oauth' AND NEW.deleted_at IS NOT NULL AND EXISTS (
        SELECT 1 FROM channels WHERE credential_id=NEW.id AND deleted_at IS NULL
    ) THEN
        RAISE EXCEPTION 'upstream credential is in use' USING ERRCODE='23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER upstream_credentials_lifecycle BEFORE UPDATE OR DELETE ON upstream_credentials
FOR EACH ROW EXECUTE FUNCTION guard_upstream_credential_lifecycle();

CREATE FUNCTION guard_codex_identity_projection() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'Codex identity projections are immutable' USING ERRCODE='23514';
END;
$$;
CREATE TRIGGER codex_identity_projection_immutable BEFORE UPDATE OR DELETE ON codex_oauth_credential_channels
FOR EACH ROW EXECUTE FUNCTION guard_codex_identity_projection();

CREATE FUNCTION sync_codex_identity_name() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    UPDATE upstream_credentials SET name=NEW.name
    WHERE id=NEW.id AND kind='codex_oauth' AND deleted_at IS NULL;
    RETURN NEW;
END;
$$;
CREATE TRIGGER channels_codex_identity_name AFTER UPDATE OF name ON channels
FOR EACH ROW WHEN (OLD.name IS DISTINCT FROM NEW.name) EXECUTE FUNCTION sync_codex_identity_name();
