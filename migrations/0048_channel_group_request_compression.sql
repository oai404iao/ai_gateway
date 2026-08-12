ALTER TABLE channel_groups
    ADD COLUMN request_compression text NOT NULL DEFAULT 'default',
    ADD CONSTRAINT channel_groups_request_compression_check
        CHECK (request_compression IN ('default', 'zstd')),
    ADD CONSTRAINT channel_groups_request_compression_format_check
        CHECK (
            request_compression = 'default'
            OR api_format = 'open_ai_responses'
        );
