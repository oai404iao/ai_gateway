-- Session client metadata lets users distinguish browsers when reviewing and
-- revoking Console login sessions. Existing sessions remain valid and gain a
-- user agent the next time their refresh credential rotates.

ALTER TABLE user_sessions
    ADD COLUMN user_agent varchar(512);
