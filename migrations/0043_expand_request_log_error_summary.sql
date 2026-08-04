-- Request logs now retain bounded upstream error response details rather than
-- only a short message extracted from structured SSE events.
ALTER TABLE request_logs
    ALTER COLUMN error_summary TYPE varchar(16384);
