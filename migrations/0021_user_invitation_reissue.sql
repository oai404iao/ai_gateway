-- Allow administrators to safely replace an expired, lost, or historically
-- broken invitation without leaving older invitation tokens usable.

ALTER TABLE user_invitations
    ADD COLUMN revoked_at timestamptz;

DROP INDEX user_invitations_user_id_active_idx;

CREATE INDEX user_invitations_user_id_active_idx
    ON user_invitations (user_id, expires_at DESC)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
