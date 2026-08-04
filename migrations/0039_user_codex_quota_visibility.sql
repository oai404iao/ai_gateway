CREATE TABLE user_group_codex_quota_visibility (
    user_group_id uuid NOT NULL
        REFERENCES user_groups (id) ON DELETE CASCADE,
    channel_group_id uuid NOT NULL
        REFERENCES channel_groups (id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_group_id, channel_group_id)
);

CREATE INDEX user_group_codex_quota_visibility_channel_group_idx
    ON user_group_codex_quota_visibility (channel_group_id);
