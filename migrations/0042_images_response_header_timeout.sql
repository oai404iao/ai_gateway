-- Images generation and edit can take substantially longer before an upstream
-- sends response headers. Give existing forwarding-policy documents a
-- dedicated Images default while preserving channel-level overrides.

UPDATE system_settings
SET value = jsonb_set(
    value,
    '{upstream,images_response_header_timeout_seconds}',
    to_jsonb(GREATEST(
        300::bigint,
        (value #>> '{upstream,response_header_timeout_seconds}')::bigint
    )),
    true
)
WHERE setting_key = 'forwarding_policy'
  AND NOT (value->'upstream' ? 'images_response_header_timeout_seconds');
