-- Durable channel automation controls and a reserved identity for periodic
-- upstream test logs. The identity is created by the application so its API
-- key secret remains unpredictable and is never present in a migration.

ALTER TABLE users
    ADD COLUMN is_system boolean NOT NULL DEFAULT false;

ALTER TABLE api_keys
    ADD COLUMN is_system boolean NOT NULL DEFAULT false;

ALTER TABLE channels
    ADD COLUMN auto_disable_allowed boolean NOT NULL DEFAULT false,
    ADD COLUMN test_model varchar(300),
    ADD CONSTRAINT channels_test_model_available
        CHECK (test_model IS NULL OR test_model = ANY (available_models));

ALTER TABLE request_logs
    ADD COLUMN request_source text NOT NULL DEFAULT 'client'
        CHECK (request_source IN ('client', 'scheduled_test'));

CREATE INDEX request_logs_scheduled_test_started_at_idx
    ON request_logs (started_at DESC, id DESC)
    WHERE request_source = 'scheduled_test';
