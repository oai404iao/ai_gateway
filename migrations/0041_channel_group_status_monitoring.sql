ALTER TABLE channel_groups
    ADD COLUMN status_statistics_enabled boolean NOT NULL DEFAULT false;

UPDATE channel_groups AS channel_group
SET status_statistics_enabled = EXISTS (
    SELECT 1
    FROM channels AS channel
    WHERE channel.channel_group_id = channel_group.id
      AND channel.status_statistics_enabled
);

CREATE INDEX channel_groups_status_statistics_enabled_idx
    ON channel_groups (id)
    WHERE status_statistics_enabled;

CREATE INDEX request_logs_channel_group_model_started_at_idx
    ON request_logs (channel_group_id, upstream_model, started_at DESC)
    WHERE channel_group_id IS NOT NULL;

DROP INDEX channels_status_statistics_enabled_idx;

ALTER TABLE channels
    DROP COLUMN status_statistics_enabled;

-- Migration 0036 created this trigger function with the former channel-level
-- monitoring column in its Images projection INSERT. Keep future Codex
-- credential projections compatible with the group-level setting.
CREATE OR REPLACE FUNCTION create_codex_credential_projections()
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
        false
    );

    INSERT INTO codex_oauth_credential_channels (credential_id, api_format, channel_id)
    VALUES (NEW.channel_id, 'open_ai_images', images_channel_id);
    RETURN NEW;
END;
$$;
