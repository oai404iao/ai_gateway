-- Console API authentication, role authorization, invitations, refresh sessions,
-- and administrator-owned API-key issuance policies.
--
-- Existing users stay valid data-plane owners after this migration. They cannot
-- log in until an operator assigns email/password credentials or creates an
-- invitation through the new Console flow.

ALTER TABLE users RENAME COLUMN name TO display_name;

ALTER TABLE users
    DROP CONSTRAINT users_status_check,
    ADD CONSTRAINT users_status_check
        CHECK (status IN ('invited', 'active', 'suspended', 'disabled')),
    ADD COLUMN email varchar(320),
    ADD COLUMN role text NOT NULL DEFAULT 'user'
        CHECK (role IN ('user', 'admin')),
    ADD COLUMN password_hash text,
    ADD COLUMN auth_version bigint NOT NULL DEFAULT 1
        CHECK (auth_version > 0),
    ADD COLUMN password_changed_at timestamptz;

CREATE UNIQUE INDEX users_email_lower_unique_idx
    ON users (lower(email))
    WHERE email IS NOT NULL;

CREATE TABLE api_key_policies (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    allowed_api_formats api_format[] NOT NULL,
    permissions text[] NOT NULL,
    allowed_group_ids uuid[],
    requests_per_minute integer,
    max_concurrent_requests integer,
    quota_limit_amount numeric(24, 8),
    max_active_keys integer NOT NULL DEFAULT 1,
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (cardinality(allowed_api_formats) > 0),
    CHECK (array_position(allowed_api_formats, NULL::api_format) IS NULL),
    CHECK (cardinality(permissions) > 0),
    CHECK (permissions <@ ARRAY['proxy', 'models.read']::text[]),
    CHECK (array_position(permissions, NULL::text) IS NULL),
    CHECK (allowed_group_ids IS NULL OR cardinality(allowed_group_ids) > 0),
    CHECK (allowed_group_ids IS NULL OR array_position(allowed_group_ids, NULL::uuid) IS NULL),
    CHECK (requests_per_minute IS NULL OR requests_per_minute > 0),
    CHECK (max_concurrent_requests IS NULL OR max_concurrent_requests > 0),
    CHECK (quota_limit_amount IS NULL OR quota_limit_amount >= 0),
    CHECK (max_active_keys > 0)
);

ALTER TABLE users
    ADD COLUMN default_api_key_policy_id uuid
        REFERENCES api_key_policies (id) ON DELETE RESTRICT;

CREATE TABLE user_sessions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    refresh_token_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    rotated_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

CREATE TABLE user_invitations (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    invited_by uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    token_hash bytea NOT NULL,
    expires_at timestamptz NOT NULL,
    accepted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    CHECK (expires_at > created_at)
);

ALTER TABLE audit_logs
    ADD COLUMN actor_role text
        CHECK (actor_role IS NULL OR actor_role IN ('user', 'admin'));

CREATE TRIGGER api_key_policies_set_updated_at
BEFORE UPDATE ON api_key_policies
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX users_default_api_key_policy_id_idx
    ON users (default_api_key_policy_id);
CREATE INDEX user_sessions_user_id_active_idx
    ON user_sessions (user_id, expires_at DESC)
    WHERE revoked_at IS NULL;
CREATE INDEX user_invitations_user_id_active_idx
    ON user_invitations (user_id, expires_at DESC)
    WHERE accepted_at IS NULL;
