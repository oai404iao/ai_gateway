ALTER TABLE channels
    ADD COLUMN status_statistics_enabled boolean NOT NULL DEFAULT false;

CREATE INDEX channels_status_statistics_enabled_idx
    ON channels (id)
    WHERE status_statistics_enabled;

CREATE INDEX request_logs_channel_model_started_at_idx
    ON request_logs (channel_id, upstream_model, started_at DESC)
    WHERE channel_id IS NOT NULL;
