-- Retain the bounded request-mode metadata used by the Console request-log
-- badges. Historical rows did not record these client request fields.

ALTER TABLE request_logs
    ADD COLUMN reasoning_effort varchar(32),
    ADD COLUMN fast_mode boolean NOT NULL DEFAULT false;
