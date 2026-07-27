-- Reusable invitation codes allow self-service Console registration without
-- storing recoverable code values. Administrators can change future
-- registration defaults, usage limits, expiry, and enabled state.

CREATE TABLE registration_invitation_codes (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    code_hash bytea NOT NULL UNIQUE,
    max_uses bigint,
    used_count bigint NOT NULL DEFAULT 0,
    expires_at timestamptz,
    enabled boolean NOT NULL DEFAULT true,
    user_group_id uuid NOT NULL
        REFERENCES user_groups (id) ON DELETE RESTRICT,
    initial_balance_amount numeric(24, 8) NOT NULL DEFAULT 0,
    created_by uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (max_uses IS NULL OR max_uses > 0),
    CHECK (used_count >= 0),
    CHECK (max_uses IS NULL OR used_count <= max_uses),
    CHECK (initial_balance_amount >= 0)
);

CREATE TRIGGER registration_invitation_codes_set_updated_at
BEFORE UPDATE ON registration_invitation_codes
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX registration_invitation_codes_user_group_id_idx
    ON registration_invitation_codes (user_group_id);
