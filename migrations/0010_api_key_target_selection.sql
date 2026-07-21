-- API-key policies now define only the channel groups and individual channels
-- that a user may choose. Per-key routing targets and admission limits remain
-- snapshots on api_keys and are chosen by the user when the key is created or
-- edited.

ALTER TABLE api_keys
    ADD COLUMN allowed_channel_ids uuid[] NOT NULL DEFAULT '{}';

DO $$
DECLARE
    constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT conname
        FROM pg_constraint
        WHERE conrelid = 'api_keys'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%allowed_group_ids%'
    LOOP
        EXECUTE format(
            'ALTER TABLE api_keys DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END;
$$;

-- Preserve the old NULL = unrestricted behavior as an explicit snapshot of
-- the groups that exist at migration time. New groups must be deliberately
-- added to a key or to the user's policy.
UPDATE api_keys
SET allowed_group_ids = ARRAY(
    SELECT g.id
    FROM channel_groups AS g
    WHERE g.api_format = ANY(api_keys.allowed_api_formats)
    ORDER BY g.id
)
WHERE allowed_group_ids IS NULL;

ALTER TABLE api_keys
    ALTER COLUMN allowed_group_ids SET DEFAULT '{}',
    ALTER COLUMN allowed_group_ids SET NOT NULL,
    ADD CONSTRAINT api_keys_allowed_group_ids_no_nulls
        CHECK (array_position(allowed_group_ids, NULL::uuid) IS NULL),
    ADD CONSTRAINT api_keys_allowed_channel_ids_no_nulls
        CHECK (array_position(allowed_channel_ids, NULL::uuid) IS NULL);

ALTER TABLE api_key_policies
    ADD COLUMN allowed_channel_ids uuid[] NOT NULL DEFAULT '{}';

DO $$
DECLARE
    constraint_name text;
BEGIN
    FOR constraint_name IN
        SELECT conname
        FROM pg_constraint
        WHERE conrelid = 'api_key_policies'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) LIKE '%allowed_group_ids%'
    LOOP
        EXECUTE format(
            'ALTER TABLE api_key_policies DROP CONSTRAINT %I',
            constraint_name
        );
    END LOOP;
END;
$$;

UPDATE api_key_policies
SET allowed_group_ids = ARRAY(
    SELECT g.id
    FROM channel_groups AS g
    WHERE g.api_format = ANY(api_key_policies.allowed_api_formats)
    ORDER BY g.id
)
WHERE allowed_group_ids IS NULL;

ALTER TABLE api_key_policies
    ALTER COLUMN allowed_group_ids SET DEFAULT '{}',
    ALTER COLUMN allowed_group_ids SET NOT NULL,
    DROP COLUMN allowed_api_formats,
    DROP COLUMN permissions,
    DROP COLUMN requests_per_minute,
    DROP COLUMN max_concurrent_requests,
    DROP COLUMN quota_limit_amount,
    DROP COLUMN max_active_keys,
    ADD CONSTRAINT api_key_policies_allowed_group_ids_no_nulls
        CHECK (array_position(allowed_group_ids, NULL::uuid) IS NULL),
    ADD CONSTRAINT api_key_policies_allowed_channel_ids_no_nulls
        CHECK (array_position(allowed_channel_ids, NULL::uuid) IS NULL);
