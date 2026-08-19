-- Preserve administrator-provided values while backfilling the connector-owned
-- Codex identity fields introduced after the privacy metadata settings.
UPDATE system_settings
SET value = jsonb_set(
    value,
    '{codex}',
    jsonb_build_object(
        'originator', 'codex_cli_rs',
        'client_version', '0.146.0',
        'user_agent', 'codex_cli_rs/0.146.0'
    ) || (value -> 'codex'),
    false
)
WHERE setting_key = 'forwarding_policy'
  AND jsonb_typeof(value -> 'codex') = 'object'
  AND (
      NOT ((value -> 'codex') ? 'originator')
      OR NOT ((value -> 'codex') ? 'client_version')
      OR NOT ((value -> 'codex') ? 'user_agent')
  );
