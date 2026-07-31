ALTER TABLE channel_groups
    DROP CONSTRAINT channel_groups_codex_oauth_responses_check,
    ADD CONSTRAINT channel_groups_codex_oauth_format_check
        CHECK (
            connector_kind <> 'codex_oauth'
            OR api_format IN ('open_ai_responses', 'open_ai_images')
        );

CREATE TABLE connector_pools (
    id uuid PRIMARY KEY,
    connector_kind text NOT NULL
        CHECK (connector_kind IN ('codex_oauth')),
    created_at timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE channel_groups
    ADD COLUMN connector_pool_id uuid REFERENCES connector_pools (id) ON DELETE RESTRICT;

INSERT INTO connector_pools (id, connector_kind)
SELECT id, connector_kind
FROM channel_groups
WHERE connector_kind = 'codex_oauth';

UPDATE channel_groups
SET connector_pool_id = id
WHERE connector_kind = 'codex_oauth';

INSERT INTO channel_groups (
    id,
    name,
    api_format,
    connector_kind,
    connector_pool_id,
    priority,
    selection_strategy,
    enabled
)
SELECT
    md5('ai-gateway:codex-images-group:' || response_group.id::text)::uuid,
    left(response_group.name, 55) || ' Images ' || response_group.id::text,
    'open_ai_images',
    'codex_oauth',
    response_group.connector_pool_id,
    response_group.priority,
    response_group.selection_strategy,
    false
FROM channel_groups AS response_group
WHERE response_group.connector_kind = 'codex_oauth'
  AND response_group.api_format = 'open_ai_responses';

ALTER TABLE channel_groups
    ADD CONSTRAINT channel_groups_connector_pool_check
        CHECK (
            (connector_kind = 'openai_compatible' AND connector_pool_id IS NULL)
            OR (connector_kind = 'codex_oauth' AND connector_pool_id IS NOT NULL)
        );

CREATE UNIQUE INDEX channel_groups_connector_pool_format_idx
    ON channel_groups (connector_pool_id, api_format)
    WHERE connector_pool_id IS NOT NULL;

CREATE FUNCTION initialize_codex_connector_pool()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    pool_kind text;
BEGIN
    IF NEW.connector_kind = 'openai_compatible' THEN
        IF NEW.connector_pool_id IS NOT NULL THEN
            RAISE EXCEPTION 'standard channel groups cannot belong to a connector pool';
        END IF;
        RETURN NEW;
    END IF;

    IF NEW.connector_kind <> 'codex_oauth' THEN
        RAISE EXCEPTION 'unsupported connector kind';
    END IF;

    IF NEW.connector_pool_id IS NULL THEN
        IF NEW.api_format <> 'open_ai_responses' THEN
            RAISE EXCEPTION 'new Codex connector pools must start with a Responses group';
        END IF;
        NEW.connector_pool_id := NEW.id;
        INSERT INTO connector_pools (id, connector_kind)
        VALUES (NEW.connector_pool_id, NEW.connector_kind);
        RETURN NEW;
    END IF;

    SELECT connector_kind
    INTO pool_kind
    FROM connector_pools
    WHERE id = NEW.connector_pool_id;

    IF pool_kind IS DISTINCT FROM NEW.connector_kind THEN
        RAISE EXCEPTION 'connector pool kind does not match the channel group';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channel_groups_initialize_codex_connector_pool
BEFORE INSERT ON channel_groups
FOR EACH ROW EXECUTE FUNCTION initialize_codex_connector_pool();

CREATE FUNCTION create_codex_images_group()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.connector_kind = 'codex_oauth'
       AND NEW.api_format = 'open_ai_responses'
       AND NOT EXISTS (
           SELECT 1
           FROM channel_groups
           WHERE connector_pool_id = NEW.connector_pool_id
             AND api_format = 'open_ai_images'
       )
    THEN
        INSERT INTO channel_groups (
            id,
            name,
            api_format,
            connector_kind,
            connector_pool_id,
            priority,
            selection_strategy,
            enabled
        )
        VALUES (
            md5('ai-gateway:codex-images-group:' || NEW.id::text)::uuid,
            left(NEW.name, 55) || ' Images ' || NEW.id::text,
            'open_ai_images',
            'codex_oauth',
            NEW.connector_pool_id,
            NEW.priority,
            NEW.selection_strategy,
            false
        );
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channel_groups_create_codex_images_group
AFTER INSERT ON channel_groups
FOR EACH ROW EXECUTE FUNCTION create_codex_images_group();

ALTER TABLE codex_oauth_credentials
    ADD COLUMN connector_pool_id uuid REFERENCES connector_pools (id) ON DELETE RESTRICT;

UPDATE codex_oauth_credentials AS credential
SET connector_pool_id = channel_group.connector_pool_id
FROM channel_groups AS channel_group
WHERE channel_group.id = credential.channel_group_id;

ALTER TABLE codex_oauth_credentials
    ALTER COLUMN connector_pool_id SET NOT NULL;

DROP INDEX codex_oauth_credentials_identity_idx;

CREATE UNIQUE INDEX codex_oauth_credentials_pool_identity_idx
    ON codex_oauth_credentials (connector_pool_id, account_id, user_id)
    NULLS NOT DISTINCT
    WHERE deleted_at IS NULL;

CREATE FUNCTION set_codex_credential_connector_pool()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    resolved_pool_id uuid;
BEGIN
    SELECT connector_pool_id
    INTO resolved_pool_id
    FROM channel_groups
    WHERE id = NEW.channel_group_id
      AND connector_kind = 'codex_oauth'
      AND api_format = 'open_ai_responses';

    IF resolved_pool_id IS NULL THEN
        RAISE EXCEPTION 'Codex credentials must retain a canonical Responses group';
    END IF;
    IF NEW.connector_pool_id IS NULL THEN
        NEW.connector_pool_id := resolved_pool_id;
    ELSIF NEW.connector_pool_id <> resolved_pool_id THEN
        RAISE EXCEPTION 'Codex credential connector pool does not match its Responses group';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER codex_oauth_credentials_set_connector_pool
BEFORE INSERT OR UPDATE OF channel_group_id, connector_pool_id
ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION set_codex_credential_connector_pool();

CREATE TABLE codex_oauth_credential_channels (
    credential_id uuid NOT NULL
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    api_format api_format NOT NULL,
    channel_id uuid NOT NULL,
    PRIMARY KEY (credential_id, api_format),
    UNIQUE (channel_id),
    FOREIGN KEY (channel_id, api_format)
        REFERENCES channels (id, api_format) ON DELETE RESTRICT,
    CHECK (api_format IN ('open_ai_responses', 'open_ai_images'))
);

INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
SELECT channel_id, 'open_ai_responses', channel_id
FROM codex_oauth_credentials;

INSERT INTO channels (
    id,
    channel_group_id,
    api_format,
    name,
    base_url,
    enabled,
    weight,
    billing_multiplier,
    proxy_id,
    override_document,
    connect_timeout_ms,
    response_header_timeout_ms,
    stream_idle_timeout_ms,
    upstream_auth_kind,
    available_models,
    status_statistics_enabled,
    auto_disable_allowed,
    supports_websocket
)
SELECT
    md5('ai-gateway:codex-images-channel:' || credential.channel_id::text)::uuid,
    images_group.id,
    'open_ai_images',
    response_channel.name,
    response_channel.base_url,
    true,
    response_channel.weight,
    response_channel.billing_multiplier,
    response_channel.proxy_id,
    '{}'::jsonb,
    response_channel.connect_timeout_ms,
    response_channel.response_header_timeout_ms,
    response_channel.stream_idle_timeout_ms,
    'none',
    ARRAY['gpt-image-2']::text[],
    false,
    false,
    false
FROM codex_oauth_credentials AS credential
JOIN channels AS response_channel
  ON response_channel.id = credential.channel_id
JOIN channel_groups AS images_group
  ON images_group.connector_pool_id = credential.connector_pool_id
 AND images_group.api_format = 'open_ai_images'
WHERE credential.deleted_at IS NULL;

INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
SELECT
    credential.channel_id,
    'open_ai_images',
    md5('ai-gateway:codex-images-channel:' || credential.channel_id::text)::uuid
FROM codex_oauth_credentials AS credential
WHERE credential.deleted_at IS NULL;

CREATE FUNCTION create_codex_credential_projections()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    images_group_id uuid;
    images_channel_id uuid;
    response_channel channels%ROWTYPE;
BEGIN
    INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
    VALUES (NEW.channel_id, 'open_ai_responses', NEW.channel_id);

    SELECT id
    INTO images_group_id
    FROM channel_groups
    WHERE connector_pool_id = NEW.connector_pool_id
      AND api_format = 'open_ai_images';

    IF images_group_id IS NULL THEN
        RAISE EXCEPTION 'Codex connector pool has no Images group';
    END IF;

    SELECT *
    INTO response_channel
    FROM channels
    WHERE id = NEW.channel_id;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'Codex credential has no Responses channel';
    END IF;

    images_channel_id :=
        md5('ai-gateway:codex-images-channel:' || NEW.channel_id::text)::uuid;

    INSERT INTO channels (
        id,
        channel_group_id,
        api_format,
        name,
        base_url,
        enabled,
        weight,
        billing_multiplier,
        proxy_id,
        override_document,
        connect_timeout_ms,
        response_header_timeout_ms,
        stream_idle_timeout_ms,
        upstream_auth_kind,
        available_models,
        status_statistics_enabled,
        auto_disable_allowed,
        supports_websocket
    )
    VALUES (
        images_channel_id,
        images_group_id,
        'open_ai_images',
        response_channel.name,
        response_channel.base_url,
        true,
        response_channel.weight,
        response_channel.billing_multiplier,
        response_channel.proxy_id,
        '{}'::jsonb,
        response_channel.connect_timeout_ms,
        response_channel.response_header_timeout_ms,
        response_channel.stream_idle_timeout_ms,
        'none',
        ARRAY['gpt-image-2']::text[],
        false,
        false,
        false
    );

    INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
    VALUES (NEW.channel_id, 'open_ai_images', images_channel_id);
    RETURN NEW;
END;
$$;

CREATE TRIGGER codex_oauth_credentials_create_projections
AFTER INSERT ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION create_codex_credential_projections();

CREATE FUNCTION sync_codex_images_projection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.api_format = 'open_ai_responses' THEN
        UPDATE channels AS images_channel
        SET name = NEW.name,
            base_url = NEW.base_url,
            weight = NEW.weight,
            billing_multiplier = NEW.billing_multiplier,
            proxy_id = NEW.proxy_id,
            connect_timeout_ms = NEW.connect_timeout_ms,
            response_header_timeout_ms = NEW.response_header_timeout_ms,
            stream_idle_timeout_ms = NEW.stream_idle_timeout_ms
        FROM codex_oauth_credential_channels AS response_projection
        JOIN codex_oauth_credential_channels AS images_projection
          ON images_projection.credential_id = response_projection.credential_id
         AND images_projection.api_format = 'open_ai_images'
        JOIN codex_oauth_credentials AS credential
          ON credential.channel_id = response_projection.credential_id
         AND credential.deleted_at IS NULL
        WHERE response_projection.channel_id = NEW.id
          AND response_projection.api_format = 'open_ai_responses'
          AND images_channel.id = images_projection.channel_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER channels_sync_codex_images_projection
AFTER UPDATE OF name, base_url, weight, billing_multiplier, proxy_id,
                connect_timeout_ms, response_header_timeout_ms, stream_idle_timeout_ms
ON channels
FOR EACH ROW EXECUTE FUNCTION sync_codex_images_projection();

CREATE FUNCTION tombstone_codex_images_projection()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN
        UPDATE channels AS images_channel
        SET enabled = false,
            name = 'deleted-codex-images-' || images_channel.id::text,
            proxy_id = NULL
        FROM codex_oauth_credential_channels AS projection
        WHERE projection.credential_id = NEW.channel_id
          AND projection.api_format = 'open_ai_images'
          AND images_channel.id = projection.channel_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER codex_oauth_credentials_tombstone_images_projection
AFTER UPDATE OF deleted_at ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION tombstone_codex_images_projection();
