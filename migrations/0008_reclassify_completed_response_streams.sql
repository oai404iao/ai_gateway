-- Older gateway versions waited for transport EOF before completing an SSE
-- request log. Responses clients commonly close immediately after the
-- protocol-level response.completed event, which incorrectly produced a
-- client_cancelled row even though terminal usage had already been observed.
--
-- For the Responses format, populated token usage is durable evidence that
-- the terminal response.completed payload was received. Repair only those
-- unambiguous historical rows and preserve genuinely interrupted streams.
ALTER TABLE request_logs DISABLE TRIGGER request_logs_prevent_mutation;

UPDATE request_logs
SET outcome = 'succeeded',
    error_code = NULL
WHERE api_format = 'open_ai_responses'
  AND streamed
  AND outcome = 'cancelled'
  AND error_code = 'client_cancelled'
  AND response_status_code BETWEEN 200 AND 299
  AND input_tokens IS NOT NULL
  AND output_tokens IS NOT NULL;

ALTER TABLE request_logs ENABLE TRIGGER request_logs_prevent_mutation;
