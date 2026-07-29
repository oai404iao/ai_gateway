ALTER TABLE channel_groups
    ADD COLUMN connector_kind text NOT NULL DEFAULT 'openai_compatible',
    ADD CONSTRAINT channel_groups_connector_kind_check
        CHECK (connector_kind IN ('openai_compatible', 'codex_oauth')),
    ADD CONSTRAINT channel_groups_codex_oauth_responses_check
        CHECK (connector_kind <> 'codex_oauth' OR api_format = 'open_ai_responses');

CREATE TABLE codex_oauth_credentials (
    channel_id uuid PRIMARY KEY REFERENCES channels (id) ON DELETE RESTRICT,
    channel_group_id uuid NOT NULL REFERENCES channel_groups (id) ON DELETE RESTRICT,
    label varchar(100) NOT NULL CHECK (btrim(label) <> ''),
    email varchar(320),
    account_id varchar(300) NOT NULL CHECK (btrim(account_id) <> ''),
    plan_type varchar(100),
    is_fedramp boolean NOT NULL DEFAULT false,
    id_token text NOT NULL CHECK (length(id_token) > 0),
    access_token text NOT NULL CHECK (length(access_token) > 0),
    refresh_token text NOT NULL CHECK (length(refresh_token) > 0),
    access_token_expires_at timestamptz,
    last_refreshed_at timestamptz NOT NULL,
    refresh_generation bigint NOT NULL DEFAULT 0 CHECK (refresh_generation >= 0),
    reauth_required boolean NOT NULL DEFAULT false,
    enabled boolean NOT NULL DEFAULT true,
    quota_threshold_percent smallint NOT NULL DEFAULT 95
        CHECK (quota_threshold_percent BETWEEN 1 AND 100),
    runtime_status text NOT NULL DEFAULT 'active'
        CHECK (runtime_status IN ('active', 'draining', 'unavailable', 'disabled')),
    quota_allowed boolean,
    quota_limit_reached boolean,
    primary_used_percent integer CHECK (primary_used_percent BETWEEN 0 AND 100),
    primary_window_seconds integer CHECK (primary_window_seconds > 0),
    primary_reset_at timestamptz,
    secondary_used_percent integer CHECK (secondary_used_percent BETWEEN 0 AND 100),
    secondary_window_seconds integer CHECK (secondary_window_seconds > 0),
    secondary_reset_at timestamptz,
    quota_checked_at timestamptz,
    last_error_code varchar(100),
    last_error_summary varchar(1000),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    CHECK (NOT reauth_required OR runtime_status IN ('unavailable', 'disabled')),
    UNIQUE (channel_group_id, account_id)
);

CREATE INDEX codex_oauth_credentials_refresh_idx
    ON codex_oauth_credentials (runtime_status, access_token_expires_at);

CREATE INDEX codex_oauth_credentials_quota_idx
    ON codex_oauth_credentials (runtime_status, quota_checked_at);

CREATE TRIGGER codex_oauth_credentials_set_updated_at
BEFORE UPDATE ON codex_oauth_credentials
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE codex_oauth_flows (
    id uuid PRIMARY KEY,
    actor_user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    channel_group_id uuid NOT NULL REFERENCES channel_groups (id) ON DELETE RESTRICT,
    label varchar(100) NOT NULL CHECK (btrim(label) <> ''),
    proxy_id uuid REFERENCES proxies (id) ON DELETE RESTRICT,
    weight integer NOT NULL CHECK (weight > 0),
    quota_threshold_percent smallint NOT NULL
        CHECK (quota_threshold_percent BETWEEN 1 AND 100),
    redirect_uri text NOT NULL
        CHECK (redirect_uri = 'http://localhost:1455/auth/callback'),
    state_hash bytea NOT NULL CHECK (octet_length(state_hash) = 32),
    code_verifier text NOT NULL CHECK (length(code_verifier) BETWEEN 43 AND 128),
    expires_at timestamptz NOT NULL,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX codex_oauth_flows_expiry_idx
    ON codex_oauth_flows (expires_at)
    WHERE completed_at IS NULL;
