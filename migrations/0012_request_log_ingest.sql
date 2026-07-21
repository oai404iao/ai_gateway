-- A low-index durable ingress table decouples accepting terminal request logs
-- from the wider query table and its secondary indexes. The application uses
-- COPY FROM for this append-only stage, then projects rows idempotently into
-- request_logs and deletes only successfully projected sequence numbers.
CREATE TABLE request_log_ingest (
    sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    request_log_id uuid NOT NULL,
    schema_version smallint NOT NULL CHECK (schema_version > 0),
    payload bytea NOT NULL,
    staged_at timestamptz NOT NULL DEFAULT now(),
    attempt_count integer NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_error_code varchar(100)
);

-- The primary-key sequence is also the projector's oldest-first access path.
-- Delayed failures are rare, so this partial scheduling index stays small.
CREATE INDEX request_log_ingest_retry_idx
    ON request_log_ingest (next_attempt_at, sequence)
    WHERE attempt_count > 0;
