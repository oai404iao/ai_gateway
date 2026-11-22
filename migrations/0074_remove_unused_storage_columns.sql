ALTER TABLE api_keys DROP COLUMN last_used_at;
ALTER TABLE request_logs DROP COLUMN streamed, DROP COLUMN attempts;
ALTER TABLE audit_logs DROP COLUMN source_ip_prefix;
