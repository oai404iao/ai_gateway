-- Retire the unused adapter without rewriting applied migration checksums.
DROP TABLE mcp_servers;
DROP TYPE mcp_server_kind;

UPDATE system_settings
SET value = value - 'mcp'
WHERE setting_key = 'forwarding_policy' AND value ? 'mcp';

-- Keep historical usage and billing, classifying it with client requests.
DROP INDEX request_logs_mcp_started_at_idx;
ALTER TABLE request_logs DISABLE TRIGGER request_logs_prevent_mutation;
UPDATE request_logs SET request_source = 'client' WHERE request_source = 'mcp';
ALTER TABLE request_logs ENABLE TRIGGER request_logs_prevent_mutation;
ALTER TABLE request_logs
    DROP CONSTRAINT request_logs_request_source_check,
    ADD CONSTRAINT request_logs_request_source_check
        CHECK (request_source IN ('client', 'scheduled_test'));
