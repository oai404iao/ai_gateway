CREATE TABLE upstream_credentials (
    id TEXT CONSTRAINT upstream_credentials_pkey PRIMARY KEY NOT NULL CHECK (ag_uuid_valid(id)),
    name TEXT NOT NULL CONSTRAINT upstream_credentials_name_check CHECK (length(trim(name)) BETWEEN 1 AND 100),
    kind TEXT NOT NULL CONSTRAINT upstream_credentials_kind_check CHECK (kind IN ('bearer','header','codex_oauth')),
    header_name TEXT,
    secret TEXT,
    allowed_base_urls TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(allowed_base_urls)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CHECK (ag_uuid_valid(revision)),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CHECK (ag_time_valid(created_at)),
    updated_at TEXT NOT NULL DEFAULT (ag_now()) CHECK (ag_time_valid(updated_at)),
    deleted_at TEXT CHECK (deleted_at IS NULL OR ag_time_valid(deleted_at)),
    CONSTRAINT upstream_credentials_scope_check CHECK (json_type(allowed_base_urls)='array'),
    CONSTRAINT upstream_credentials_auth_check CHECK (
        (deleted_at IS NOT NULL AND secret IS NULL AND NOT enabled)
        OR (deleted_at IS NULL AND (
            (kind='codex_oauth' AND secret IS NULL AND header_name IS NULL AND allowed_base_urls='[]')
            OR (kind='bearer' AND secret IS NOT NULL AND length(trim(secret))>0
                AND header_name IS NULL AND json_array_length(allowed_base_urls)>0)
            OR (kind='header' AND secret IS NOT NULL AND length(trim(secret))>0
                AND header_name IS NOT NULL AND json_array_length(allowed_base_urls)>0)
        ))
    )
) STRICT;
ALTER TABLE channels ADD COLUMN credential_id TEXT
    CONSTRAINT channels_credential_id_fkey REFERENCES upstream_credentials(id) ON DELETE RESTRICT
    CHECK (credential_id IS NULL OR ag_uuid_valid(credential_id))
    CONSTRAINT channels_auth_externalized_check CHECK (credential_id IS NULL OR (
        upstream_auth_kind='none' AND upstream_auth_header_name IS NULL AND upstream_api_key IS NULL));
ALTER TABLE channels ADD COLUMN credential_binding_revision TEXT NOT NULL
    DEFAULT '00000000-0000-0000-0000-000000000000' CHECK (ag_uuid_valid(credential_binding_revision));
CREATE INDEX channels_credential_id_idx ON channels(credential_id) WHERE credential_id IS NOT NULL;

INSERT INTO upstream_credentials(id,name,kind,header_name,secret,allowed_base_urls,created_at,updated_at)
SELECT id,name,upstream_auth_kind,upstream_auth_header_name,upstream_api_key,
       json_array(base_url),created_at,updated_at
FROM channels WHERE deleted_at IS NULL AND upstream_auth_kind<>'none';
INSERT INTO upstream_credentials(id,name,kind,enabled,created_at,updated_at,deleted_at)
SELECT co.channel_id,c.name,'codex_oauth',co.enabled AND co.deleted_at IS NULL,
       co.created_at,co.updated_at,co.deleted_at
FROM codex_oauth_credentials co JOIN channels c ON c.id=co.channel_id;
UPDATE channels SET credential_id=id,
    upstream_auth_kind='none',upstream_auth_header_name=NULL,upstream_api_key=NULL,updated_at=ag_now()
WHERE deleted_at IS NULL AND upstream_auth_kind<>'none';
UPDATE channels SET credential_id=(
    SELECT credential_id FROM codex_oauth_credential_channels WHERE channel_id=channels.id),updated_at=ag_now()
WHERE deleted_at IS NULL AND id IN (SELECT channel_id FROM codex_oauth_credential_channels);

CREATE TRIGGER channels_auth_externalized_insert BEFORE INSERT ON channels
WHEN NEW.upstream_auth_kind<>'none' OR NEW.upstream_auth_header_name IS NOT NULL OR NEW.upstream_api_key IS NOT NULL
BEGIN SELECT RAISE(ABORT,'channels_auth_externalized_check'); END;
CREATE TRIGGER channels_auth_externalized_update BEFORE UPDATE ON channels
WHEN NEW.upstream_auth_kind<>'none' OR NEW.upstream_auth_header_name IS NOT NULL OR NEW.upstream_api_key IS NOT NULL
BEGIN SELECT RAISE(ABORT,'channels_auth_externalized_check'); END;

CREATE TRIGGER codex_identity_insert BEFORE INSERT ON codex_oauth_credentials BEGIN
    INSERT INTO upstream_credentials(id,name,kind,enabled,created_at,updated_at,deleted_at)
    SELECT NEW.channel_id,c.name,'codex_oauth',NEW.enabled AND NEW.deleted_at IS NULL,
           NEW.created_at,NEW.updated_at,NEW.deleted_at
    FROM channels c WHERE c.id=NEW.channel_id;
END;
CREATE TRIGGER codex_identity_update BEFORE UPDATE ON codex_oauth_credentials BEGIN
    UPDATE upstream_credentials SET enabled=NEW.enabled AND NEW.deleted_at IS NULL,
        deleted_at=NEW.deleted_at,updated_at=ag_now(),
        revision=CASE WHEN OLD.enabled IS NOT NEW.enabled OR OLD.deleted_at IS NOT NEW.deleted_at
            OR OLD.access_token IS NOT NEW.access_token
            THEN ag_md5_uuid(hex(randomblob(32))) ELSE revision END
    WHERE id=NEW.channel_id;
END;
CREATE TRIGGER codex_projection_identity AFTER INSERT ON codex_oauth_credential_channels BEGIN
    UPDATE channels SET credential_id=NEW.credential_id,updated_at=ag_now() WHERE id=NEW.channel_id;
END;

CREATE TRIGGER channels_credential_revision AFTER UPDATE OF credential_id ON channels
WHEN OLD.credential_id IS NOT NEW.credential_id AND NEW.deleted_at IS NULL BEGIN
    UPDATE channels SET credential_binding_revision=ag_md5_uuid(hex(randomblob(32))),updated_at=ag_now()
    WHERE id=NEW.id;
END;

CREATE TRIGGER channels_credential_binding_insert BEFORE INSERT ON channels
WHEN NEW.credential_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM upstream_credentials u JOIN channel_groups g ON g.id=NEW.channel_group_id
    WHERE u.id=NEW.credential_id AND
      ((u.kind<>'codex_oauth' AND u.deleted_at IS NULL AND g.connector_kind='openai_compatible')
       OR (u.kind='codex_oauth' AND g.connector_kind='codex_oauth' AND EXISTS (
           SELECT 1 FROM codex_oauth_credential_channels p
           WHERE p.channel_id=NEW.id AND p.credential_id=NEW.credential_id)))
) BEGIN SELECT RAISE(ABORT,'channels_credential_binding'); END;
CREATE TRIGGER channels_credential_binding_update BEFORE UPDATE OF credential_id,channel_group_id ON channels
WHEN (NEW.credential_id IS NULL AND EXISTS (
    SELECT 1 FROM codex_oauth_credential_channels WHERE channel_id=NEW.id))
OR (NEW.credential_id IS NOT NULL AND NOT EXISTS (
    SELECT 1 FROM upstream_credentials u JOIN channel_groups g ON g.id=NEW.channel_group_id
    WHERE u.id=NEW.credential_id AND
      ((u.kind<>'codex_oauth' AND u.deleted_at IS NULL AND g.connector_kind='openai_compatible')
       OR (u.kind='codex_oauth' AND g.connector_kind='codex_oauth' AND EXISTS (
           SELECT 1 FROM codex_oauth_credential_channels p
           WHERE p.channel_id=NEW.id AND p.credential_id=NEW.credential_id)))
)) BEGIN SELECT RAISE(ABORT,'channels_credential_binding'); END;

CREATE TRIGGER upstream_credentials_lifecycle BEFORE UPDATE ON upstream_credentials
WHEN OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind OR (
    NEW.kind<>'codex_oauth' AND NEW.deleted_at IS NOT NULL AND EXISTS (
        SELECT 1 FROM channels WHERE credential_id=NEW.id AND deleted_at IS NULL
    )
) BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;
CREATE TRIGGER upstream_credentials_no_delete BEFORE DELETE ON upstream_credentials
BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;

CREATE TRIGGER codex_identity_projection_no_update BEFORE UPDATE ON codex_oauth_credential_channels
BEGIN SELECT RAISE(ABORT,'codex_identity_projection_immutable'); END;
CREATE TRIGGER codex_identity_projection_no_delete BEFORE DELETE ON codex_oauth_credential_channels
BEGIN SELECT RAISE(ABORT,'codex_identity_projection_immutable'); END;
CREATE TRIGGER channels_codex_identity_name AFTER UPDATE OF name ON channels
WHEN OLD.name IS NOT NEW.name BEGIN
    UPDATE upstream_credentials SET name=NEW.name,updated_at=ag_now()
    WHERE id=NEW.id AND kind='codex_oauth' AND deleted_at IS NULL;
END;
