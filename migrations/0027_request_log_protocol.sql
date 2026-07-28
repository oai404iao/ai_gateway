-- Record the client-visible request transport independently from the legacy
-- streamed flag so Responses WebSocket requests are distinguishable from SSE.
--
-- Historical streamed rows cannot be separated into SSE and WebSocket after
-- the fact, so the migration classifies them as SSE. New gateway writes record
-- WebSocket requests explicitly.

ALTER TABLE request_logs
    ADD COLUMN request_protocol text NOT NULL DEFAULT 'non_stream'
        CHECK (request_protocol IN ('non_stream', 'sse', 'websocket'));

ALTER TABLE request_logs DISABLE TRIGGER request_logs_prevent_mutation;

UPDATE request_logs
SET request_protocol = 'sse'
WHERE streamed;

ALTER TABLE request_logs ENABLE TRIGGER request_logs_prevent_mutation;
