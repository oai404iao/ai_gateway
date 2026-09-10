-- Sharing membership belongs to explicit seats, not to a user's ordinary
-- authorization group. Existing seat arrays and every monetary table remain
-- unchanged.
CREATE OR REPLACE FUNCTION protect_codex_sharing_binding() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id <> OLD.id OR NEW.credential_id <> OLD.credential_id
       OR NEW.provider_account_id <> OLD.provider_account_id
       OR NEW.provider_user_id <> OLD.provider_user_id
       OR jsonb_array_length(NEW.seats) < jsonb_array_length(OLD.seats)
    THEN
        RAISE EXCEPTION 'sharing bindings and existing seat numbers are immutable';
    END IF;
    RETURN NEW;
END;
$$;

-- Before group targets stop implicitly granting a bound credential, preserve
-- every existing Key's effective sharing access as explicit canonical channel
-- targets. Do not add a paired format that the Key did not previously select.
UPDATE api_keys k
SET allowed_channel_ids = (
    SELECT ARRAY(
        SELECT DISTINCT channel_id
        FROM (
            SELECT unnest(k.allowed_channel_ids) AS channel_id
            UNION ALL
            SELECT projection.channel_id
            FROM codex_sharing_groups sharing
            JOIN codex_oauth_credential_channels projection
              ON projection.credential_id=sharing.credential_id
            JOIN channels channel ON channel.id=projection.channel_id
            WHERE sharing.seats @> jsonb_build_array(k.user_id)
              AND channel.channel_group_id=ANY(k.allowed_group_ids)
        ) targets
        ORDER BY channel_id
    )
)
WHERE EXISTS (
    SELECT 1
    FROM codex_sharing_groups sharing
    JOIN codex_oauth_credential_channels projection
      ON projection.credential_id=sharing.credential_id
    JOIN channels channel ON channel.id=projection.channel_id
    WHERE sharing.seats @> jsonb_build_array(k.user_id)
      AND channel.channel_group_id=ANY(k.allowed_group_ids)
      AND NOT projection.channel_id=ANY(k.allowed_channel_ids)
);

ALTER TABLE codex_sharing_groups
    DROP COLUMN user_group_id;
