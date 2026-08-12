-- User groups can silently remove client-requested Fast/Priority processing.
-- The setting is compiled into every member API key so the data plane keeps
-- using immutable snapshots without per-request database reads.

ALTER TABLE user_groups
    ADD COLUMN filter_fast_mode boolean NOT NULL DEFAULT false;
