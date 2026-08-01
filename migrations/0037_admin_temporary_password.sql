-- Administrator-assisted Console password recovery.
--
-- An issued temporary password replaces the previous Console password,
-- revokes every existing Console session, and can only create a
-- password-change session. The account remains locked in that flow after the
-- temporary password expires until an administrator issues another one.

ALTER TABLE users
    ADD COLUMN password_change_required boolean NOT NULL DEFAULT false,
    ADD COLUMN temporary_password_issued_at timestamptz,
    ADD COLUMN temporary_password_expires_at timestamptz,
    ADD CONSTRAINT users_temporary_password_state_check
        CHECK (
            (
                password_change_required
                AND password_hash IS NOT NULL
                AND temporary_password_issued_at IS NOT NULL
                AND temporary_password_expires_at IS NOT NULL
                AND temporary_password_expires_at > temporary_password_issued_at
            )
            OR
            (
                NOT password_change_required
                AND temporary_password_issued_at IS NULL
                AND temporary_password_expires_at IS NULL
            )
        );

ALTER TABLE user_sessions
    ADD COLUMN purpose text NOT NULL DEFAULT 'normal'
        CHECK (purpose IN ('normal', 'password_change'));
