-- Add the required session-affinity section to existing forwarding-policy
-- documents without changing the generic system_settings table shape.

UPDATE system_settings
SET value = jsonb_set(
    value,
    '{session_affinity}',
    '{
      "enabled": false,
      "max_entries": 100000,
      "default_ttl_seconds": 3600,
      "rules": []
    }'::jsonb,
    true
)
WHERE setting_key = 'forwarding_policy'
  AND NOT (value ? 'session_affinity');
