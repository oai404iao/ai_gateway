CREATE TYPE api_format AS ENUM (
    'open_ai_chat_completions',
    'open_ai_responses'
);

CREATE FUNCTION set_updated_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$;

CREATE TABLE users (
    id uuid PRIMARY KEY,
    name varchar(200) NOT NULL UNIQUE,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended', 'disabled')),
    balance_amount numeric(24, 8) NOT NULL DEFAULT 0,
    currency char(3) NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE models (
    id uuid PRIMARY KEY,
    source_model_id varchar(300) NOT NULL UNIQUE,
    display_name varchar(300) NOT NULL,
    provider_name varchar(200),
    enabled boolean NOT NULL DEFAULT true,
    currency char(3) NOT NULL,
    price_unit_tokens bigint NOT NULL CHECK (price_unit_tokens > 0),
    input_unit_price numeric(24, 12) NOT NULL CHECK (input_unit_price >= 0),
    cached_input_unit_price numeric(24, 12) NOT NULL CHECK (cached_input_unit_price >= 0),
    cache_write_unit_price numeric(24, 12) NOT NULL CHECK (cache_write_unit_price >= 0),
    output_unit_price numeric(24, 12) NOT NULL CHECK (output_unit_price >= 0),
    price_effective_at timestamptz NOT NULL,
    source_payload jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(source_payload) = 'object'),
    last_synced_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE channel_groups (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    api_format api_format NOT NULL,
    priority integer NOT NULL CHECK (priority >= 0),
    selection_strategy text NOT NULL CHECK (selection_strategy IN ('weighted_random', 'weighted_round_robin')),
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (id, api_format)
);

CREATE TABLE proxies (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    proxy_url text NOT NULL CHECK (proxy_url ~* '^(https?|socks[45]?)://'),
    username text,
    password text,
    no_proxy_hosts text[] NOT NULL DEFAULT '{}',
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE config_templates (
    id uuid PRIMARY KEY,
    name varchar(100) NOT NULL UNIQUE,
    description text,
    document jsonb NOT NULL CHECK (jsonb_typeof(document) = 'object'),
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE api_keys (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    name varchar(100) NOT NULL,
    secret_value text NOT NULL UNIQUE,
    status text NOT NULL CHECK (status IN ('active', 'disabled', 'revoked', 'expired')),
    expires_at timestamptz,
    allowed_api_formats api_format[] NOT NULL,
    permissions text[] NOT NULL,
    allowed_group_ids uuid[],
    requests_per_minute integer,
    tokens_per_minute integer,
    max_concurrent_requests integer,
    quota_limit_amount numeric(24, 8),
    quota_used_amount numeric(24, 8) NOT NULL DEFAULT 0,
    last_used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, name),
    CHECK (expires_at IS NULL OR expires_at > created_at),
    CHECK (cardinality(allowed_api_formats) > 0),
    CHECK (array_position(allowed_api_formats, NULL::api_format) IS NULL),
    CHECK (permissions <@ ARRAY['proxy', 'models.read']::text[]),
    CHECK (array_position(permissions, NULL::text) IS NULL),
    CHECK (allowed_group_ids IS NULL OR cardinality(allowed_group_ids) > 0),
    CHECK (allowed_group_ids IS NULL OR array_position(allowed_group_ids, NULL::uuid) IS NULL),
    CHECK (requests_per_minute IS NULL OR requests_per_minute > 0),
    CHECK (tokens_per_minute IS NULL OR tokens_per_minute > 0),
    CHECK (max_concurrent_requests IS NULL OR max_concurrent_requests > 0),
    CHECK (quota_limit_amount IS NULL OR quota_limit_amount >= 0),
    CHECK (quota_used_amount >= 0),
    CHECK (quota_limit_amount IS NULL OR quota_used_amount <= quota_limit_amount)
);

CREATE TABLE model_rules (
    id uuid PRIMARY KEY,
    client_model varchar(300) NOT NULL,
    api_format api_format NOT NULL,
    model_id uuid NOT NULL REFERENCES models (id) ON DELETE RESTRICT,
    upstream_model varchar(300) NOT NULL,
    channel_group_ids uuid[] NOT NULL DEFAULT '{}',
    channel_ids uuid[] NOT NULL DEFAULT '{}',
    enabled boolean NOT NULL DEFAULT true,
    description text,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (client_model, api_format),
    CHECK (cardinality(channel_group_ids) + cardinality(channel_ids) > 0)
);

CREATE TABLE channels (
    id uuid PRIMARY KEY,
    channel_group_id uuid NOT NULL,
    api_format api_format NOT NULL,
    name varchar(100) NOT NULL,
    base_url text NOT NULL CHECK (base_url ~* '^https?://'),
    enabled boolean NOT NULL DEFAULT true,
    auto_disabled boolean NOT NULL DEFAULT false,
    auto_disabled_reason varchar(500),
    weight integer NOT NULL CHECK (weight > 0),
    proxy_id uuid REFERENCES proxies (id) ON DELETE RESTRICT,
    config_template_id uuid REFERENCES config_templates (id) ON DELETE RESTRICT,
    override_document jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(override_document) = 'object'),
    connect_timeout_ms integer,
    response_header_timeout_ms integer,
    stream_idle_timeout_ms integer,
    upstream_auth_kind text NOT NULL CHECK (upstream_auth_kind IN ('none', 'bearer', 'header')),
    upstream_auth_header_name varchar(100),
    upstream_api_key text,
    available_models text[] NOT NULL DEFAULT '{}',
    health_check jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(health_check) = 'object'),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (channel_group_id, name),
    UNIQUE (id, api_format),
    FOREIGN KEY (channel_group_id, api_format)
        REFERENCES channel_groups (id, api_format) ON DELETE RESTRICT,
    CHECK (auto_disabled OR auto_disabled_reason IS NULL),
    CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms > 0),
    CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms > 0),
    CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms > 0),
    CHECK (
        (upstream_auth_kind = 'none' AND upstream_auth_header_name IS NULL AND upstream_api_key IS NULL)
        OR (upstream_auth_kind = 'bearer' AND upstream_auth_header_name IS NULL AND upstream_api_key IS NOT NULL)
        OR (upstream_auth_kind = 'header' AND upstream_auth_header_name IS NOT NULL AND upstream_api_key IS NOT NULL)
    ),
    CHECK (
        upstream_auth_header_name IS NULL
        OR lower(upstream_auth_header_name) NOT IN (
            'host', 'content-length', 'connection', 'transfer-encoding', 'authorization',
            'proxy-authorization', 'proxy-authenticate', 'keep-alive', 'te', 'trailer', 'upgrade',
            'proxy-connection'
        )
    )
);

CREATE TABLE request_logs (
    id uuid PRIMARY KEY,
    started_at timestamptz NOT NULL,
    completed_at timestamptz NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE RESTRICT,
    api_key_id uuid NOT NULL REFERENCES api_keys (id) ON DELETE RESTRICT,
    api_format api_format NOT NULL,
    client_model varchar(300) NOT NULL,
    upstream_model varchar(300),
    model_rule_id uuid REFERENCES model_rules (id) ON DELETE RESTRICT,
    channel_group_id uuid REFERENCES channel_groups (id) ON DELETE RESTRICT,
    channel_id uuid REFERENCES channels (id) ON DELETE RESTRICT,
    outcome text NOT NULL CHECK (outcome IN ('succeeded', 'failed', 'rejected', 'cancelled')),
    response_status_code smallint CHECK (response_status_code BETWEEN 100 AND 599),
    streamed boolean NOT NULL DEFAULT false,
    ttft_ms integer CHECK (ttft_ms >= 0),
    total_duration_ms integer CHECK (total_duration_ms >= 0),
    output_tokens_per_second numeric(14, 4) CHECK (output_tokens_per_second >= 0),
    input_tokens bigint CHECK (input_tokens >= 0),
    cached_input_tokens bigint CHECK (cached_input_tokens >= 0),
    cache_write_tokens bigint CHECK (cache_write_tokens >= 0),
    output_tokens bigint CHECK (output_tokens >= 0),
    model_id uuid REFERENCES models (id) ON DELETE RESTRICT,
    currency char(3),
    price_unit_tokens bigint CHECK (price_unit_tokens > 0),
    price_effective_at timestamptz,
    input_unit_price numeric(24, 12) CHECK (input_unit_price >= 0),
    cached_input_unit_price numeric(24, 12) CHECK (cached_input_unit_price >= 0),
    cache_write_unit_price numeric(24, 12) CHECK (cache_write_unit_price >= 0),
    output_unit_price numeric(24, 12) CHECK (output_unit_price >= 0),
    cost_amount numeric(24, 8) CHECK (cost_amount >= 0),
    attempts jsonb NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(attempts) = 'array'),
    error_code varchar(100),
    error_summary varchar(1000),
    billed_at timestamptz,
    CHECK (completed_at >= started_at),
    CHECK (cached_input_tokens IS NULL OR (input_tokens IS NOT NULL AND cached_input_tokens <= input_tokens)),
    CHECK (cache_write_tokens IS NULL OR (input_tokens IS NOT NULL AND cache_write_tokens <= input_tokens)),
    CHECK (
        (currency IS NULL AND price_unit_tokens IS NULL AND price_effective_at IS NULL
            AND input_unit_price IS NULL AND cached_input_unit_price IS NULL
            AND cache_write_unit_price IS NULL AND output_unit_price IS NULL)
        OR
        (currency IS NOT NULL AND price_unit_tokens IS NOT NULL AND price_effective_at IS NOT NULL
            AND input_unit_price IS NOT NULL AND cached_input_unit_price IS NOT NULL
            AND cache_write_unit_price IS NOT NULL AND output_unit_price IS NOT NULL)
    ),
    CHECK (
        billed_at IS NULL
        OR (
            cost_amount IS NOT NULL
            AND model_id IS NOT NULL
            AND currency IS NOT NULL
            AND price_unit_tokens IS NOT NULL
            AND price_effective_at IS NOT NULL
            AND input_unit_price IS NOT NULL
            AND cached_input_unit_price IS NOT NULL
            AND cache_write_unit_price IS NOT NULL
            AND output_unit_price IS NOT NULL
        )
    )
);

CREATE TABLE audit_logs (
    id uuid PRIMARY KEY,
    occurred_at timestamptz NOT NULL DEFAULT now(),
    actor_user_id uuid REFERENCES users (id) ON DELETE RESTRICT,
    actor_type text NOT NULL CHECK (actor_type IN ('user', 'system')),
    action varchar(100) NOT NULL,
    object_type text NOT NULL,
    object_id uuid NOT NULL,
    before_redacted jsonb,
    after_redacted jsonb,
    correlation_id varchar(100),
    reason varchar(500),
    source_ip_prefix cidr,
    CHECK (before_redacted IS NULL OR jsonb_typeof(before_redacted) = 'object'),
    CHECK (after_redacted IS NULL OR jsonb_typeof(after_redacted) = 'object')
);

CREATE TABLE system_settings (
    setting_key varchar(100) PRIMARY KEY,
    value jsonb NOT NULL CHECK (jsonb_typeof(value) = 'object'),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE FUNCTION prevent_log_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_TABLE_NAME = 'audit_logs' THEN
        RAISE EXCEPTION 'audit_logs are append-only';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;

    IF OLD.billed_at IS NOT NULL
       OR NEW.billed_at IS NULL
       OR (to_jsonb(NEW) - 'billed_at') IS DISTINCT FROM (to_jsonb(OLD) - 'billed_at') THEN
        RAISE EXCEPTION 'request_logs may only be updated once to set billed_at';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER users_set_updated_at
BEFORE UPDATE ON users
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER api_keys_set_updated_at
BEFORE UPDATE ON api_keys
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER models_set_updated_at
BEFORE UPDATE ON models
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER model_rules_set_updated_at
BEFORE UPDATE ON model_rules
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER channel_groups_set_updated_at
BEFORE UPDATE ON channel_groups
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER channels_set_updated_at
BEFORE UPDATE ON channels
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER proxies_set_updated_at
BEFORE UPDATE ON proxies
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER config_templates_set_updated_at
BEFORE UPDATE ON config_templates
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER system_settings_set_updated_at
BEFORE UPDATE ON system_settings
FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER request_logs_prevent_mutation
BEFORE UPDATE OR DELETE ON request_logs
FOR EACH ROW EXECUTE FUNCTION prevent_log_mutation();

CREATE TRIGGER audit_logs_prevent_mutation
BEFORE UPDATE OR DELETE ON audit_logs
FOR EACH ROW EXECUTE FUNCTION prevent_log_mutation();

CREATE INDEX api_keys_user_id_status_idx ON api_keys (user_id, status);
CREATE INDEX channels_channel_group_id_enabled_idx ON channels (channel_group_id, enabled);
CREATE INDEX request_logs_api_key_id_started_at_idx ON request_logs (api_key_id, started_at DESC);
CREATE INDEX request_logs_user_id_started_at_idx ON request_logs (user_id, started_at DESC);
CREATE INDEX request_logs_channel_id_started_at_idx ON request_logs (channel_id, started_at DESC);
CREATE INDEX request_logs_failed_started_at_idx ON request_logs (started_at DESC)
    WHERE outcome = 'failed';
CREATE INDEX audit_logs_object_occurred_at_idx ON audit_logs (object_type, object_id, occurred_at DESC);
CREATE INDEX audit_logs_actor_occurred_at_idx ON audit_logs (actor_user_id, occurred_at DESC);
