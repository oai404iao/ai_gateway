-- Replace client-local Codex workspace fingerprints with a configurable,
-- privacy-preserving system projection.
UPDATE system_settings
SET value = jsonb_set(
    value,
    '{codex}',
    jsonb_build_object(
        'workspace_path', '/workspace',
        'git_remote_url', 'https://github.com/oai404iao/ai_gateway'
    ),
    true
)
WHERE setting_key = 'forwarding_policy'
  AND NOT (value ? 'codex');
