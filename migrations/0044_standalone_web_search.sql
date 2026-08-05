-- Standalone Codex web search reuses Responses routing and credentials, but
-- only channels which explicitly advertise the alpha/search endpoint are
-- eligible.
ALTER TABLE channels
    ADD COLUMN supports_standalone_web_search boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT channels_standalone_web_search_api_format_check
        CHECK (
            NOT supports_standalone_web_search
            OR api_format = 'open_ai_responses'
        );

-- ChatGPT/Codex OAuth Responses channels expose alpha/search through the same
-- provider base URL and credential projection.
UPDATE channels AS channel
SET supports_standalone_web_search = true
FROM channel_groups AS channel_group
WHERE channel_group.id = channel.channel_group_id
  AND channel_group.connector_kind = 'codex_oauth'
  AND channel.api_format = 'open_ai_responses';

ALTER TABLE request_logs
    DROP CONSTRAINT request_logs_check5,
    ADD CONSTRAINT request_logs_api_operation_format_check
        CHECK (
            (api_format = 'open_ai_chat_completions' AND api_operation = 'chat_completions')
            OR (
                api_format = 'open_ai_responses'
                AND api_operation IN ('responses', 'standalone_web_search')
            )
            OR (
                api_format = 'open_ai_images'
                AND api_operation IN ('images_generation', 'images_edit')
            )
        );

-- The endpoint is unary and can perform several upstream web operations before
-- returning response headers. Preserve a longer default while retaining
-- channel-level overrides.
UPDATE system_settings
SET value = jsonb_set(
    value,
    '{upstream,standalone_web_search_response_header_timeout_seconds}',
    to_jsonb(GREATEST(
        300::bigint,
        (value #>> '{upstream,response_header_timeout_seconds}')::bigint
    )),
    true
)
WHERE setting_key = 'forwarding_policy'
  AND NOT (
      value->'upstream'
      ? 'standalone_web_search_response_header_timeout_seconds'
  );
