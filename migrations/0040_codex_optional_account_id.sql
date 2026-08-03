ALTER TABLE codex_oauth_credentials
    ALTER COLUMN account_id DROP NOT NULL,
    ADD CONSTRAINT codex_oauth_credentials_account_or_user_check
        CHECK (account_id IS NOT NULL OR user_id IS NOT NULL);
