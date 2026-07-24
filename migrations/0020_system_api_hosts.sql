UPDATE system_settings
SET value = value || '{"api_hosts":[]}'::jsonb
WHERE setting_key = 'forwarding_policy'
  AND NOT value ? 'api_hosts';
