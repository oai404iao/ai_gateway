ALTER TABLE codex_oauth_credentials
    ADD COLUMN user_id varchar(300),
    ADD COLUMN deleted_at timestamptz,
    ADD CONSTRAINT codex_oauth_credentials_user_id_check
        CHECK (user_id IS NULL OR btrim(user_id) <> '');

ALTER TABLE codex_oauth_credentials
    DROP CONSTRAINT codex_oauth_credentials_channel_group_id_account_id_key;

CREATE UNIQUE INDEX codex_oauth_credentials_identity_idx
    ON codex_oauth_credentials (channel_group_id, account_id, user_id)
    NULLS NOT DISTINCT
    WHERE deleted_at IS NULL;
