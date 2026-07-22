-- Rename the retry limit so it counts only automatic retries after the
-- initial request. Preserve the effective total-attempt limit for existing
-- values where possible.

UPDATE system_settings
SET value = jsonb_set(
    value #- '{request_retry,max_attempts}',
    '{request_retry,max_retries}',
    to_jsonb(
      GREATEST(
        ((value #>> '{request_retry,max_attempts}')::integer - 1),
        1
      )
    ),
    true
)
WHERE setting_key = 'forwarding_policy'
  AND value #> '{request_retry,max_attempts}' IS NOT NULL;
