-- Responses WebSocket forwarding is opt-in at the system, user, and channel
-- layers. Existing and newly created records remain disabled until explicitly
-- enabled through the Console.

ALTER TABLE users
    ADD COLUMN websocket_enabled boolean NOT NULL DEFAULT false;

ALTER TABLE channels
    ADD COLUMN supports_websocket boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT channels_websocket_api_format_check
        CHECK (NOT supports_websocket OR api_format = 'open_ai_responses');

UPDATE system_settings
SET value = value || jsonb_build_object(
    'websocket',
    jsonb_build_object(
        'enabled', false,
        'max_idle_connections', 128,
        'idle_timeout_seconds', 300,
        'max_connection_age_seconds', 3300
    )
)
WHERE setting_key = 'forwarding_policy'
  AND NOT value ? 'websocket';
