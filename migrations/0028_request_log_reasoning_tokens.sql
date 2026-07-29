ALTER TABLE request_logs
    ADD COLUMN reasoning_tokens bigint CHECK (reasoning_tokens >= 0),
    ADD CONSTRAINT request_logs_reasoning_tokens_within_output
        CHECK (
            reasoning_tokens IS NULL
            OR (output_tokens IS NOT NULL AND reasoning_tokens <= output_tokens)
        );
