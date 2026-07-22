-- Breaking cleanup of control-plane fields that no longer have runtime
-- semantics. Existing values are intentionally discarded.
ALTER TABLE api_keys DROP COLUMN tokens_per_minute;
ALTER TABLE channels DROP COLUMN health_check;
