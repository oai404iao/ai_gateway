-- Add pre-header request failover defaults to existing forwarding-policy
-- documents without changing the generic system_settings table shape.

UPDATE system_settings
SET value = jsonb_set(
    value,
    '{request_retry}',
    '{
      "enabled": true,
      "max_attempts": 2
    }'::jsonb,
    true
)
WHERE setting_key = 'forwarding_policy'
  AND NOT (value ? 'request_retry');
