-- User groups provide one shared default API-key policy while preserving an
-- optional per-user policy override. Deletion remains audit-safe by
-- anonymizing the user row instead of removing request-log ownership.

CREATE TABLE user_groups (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    description varchar(500),
    default_api_key_policy_id uuid
        REFERENCES api_key_policies (id) ON DELETE RESTRICT,
    system_role text UNIQUE
        CHECK (system_role IS NULL OR system_role IN ('user', 'admin')),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO user_groups (id, name, description, system_role)
VALUES
    (
        '00000000-0000-0000-0000-000000000101',
        'Default Users',
        'Default group for newly invited users.',
        'user'
    ),
    (
        '00000000-0000-0000-0000-000000000102',
        'Default Administrators',
        'Default group for newly invited administrators.',
        'admin'
    );

ALTER TABLE users
    ADD COLUMN user_group_id uuid,
    ADD COLUMN deleted_at timestamptz,
    ADD COLUMN deleted_by uuid REFERENCES users (id) ON DELETE RESTRICT;

UPDATE users
SET user_group_id = CASE role
    WHEN 'admin' THEN '00000000-0000-0000-0000-000000000102'::uuid
    ELSE '00000000-0000-0000-0000-000000000101'::uuid
END;

ALTER TABLE users
    ALTER COLUMN user_group_id
        SET DEFAULT '00000000-0000-0000-0000-000000000101'::uuid,
    ALTER COLUMN user_group_id SET NOT NULL,
    ADD CONSTRAINT users_user_group_id_fkey
        FOREIGN KEY (user_group_id) REFERENCES user_groups (id) ON DELETE RESTRICT;

CREATE TRIGGER user_groups_set_updated_at
BEFORE UPDATE ON user_groups
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX users_user_group_id_idx
    ON users (user_group_id)
    WHERE deleted_at IS NULL;
CREATE INDEX users_deleted_at_idx
    ON users (deleted_at)
    WHERE deleted_at IS NOT NULL;
CREATE INDEX user_groups_default_api_key_policy_id_idx
    ON user_groups (default_api_key_policy_id);
