-- SQLite baseline corresponding to the PostgreSQL schema after migration 0049.
--
-- This is a new, independent migration history: no SQLite database was
-- released before this baseline. Future schema changes must add a migration to
-- both backend histories rather than editing this file.
--
-- Portable storage conventions:
--   * UUIDs are 16-byte BLOBs; UTC timestamps are RFC 3339 TEXT.
--   * JSON objects and PostgreSQL arrays are canonical JSON TEXT.
--   * exact NUMERIC values are decimal TEXT to avoid SQLite REAL coercion.
--   * booleans are INTEGER constrained to 0 or 1.
--
-- Repository adapters, not SQL casts, must perform exact-decimal arithmetic.
-- PostgreSQL-only Codex projection functions are intentionally not reproduced
-- here; the SQLite repository port must execute those mutations explicitly in
-- the surrounding transaction.

CREATE TABLE api_key_policies (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    allowed_group_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(allowed_group_ids) AND json_type(allowed_group_ids) = 'array'),
    allowed_channel_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(allowed_channel_ids) AND json_type(allowed_channel_ids) = 'array'),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE user_groups (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    description TEXT CHECK (description IS NULL OR length(description) <= 500),
    default_api_key_policy_id BLOB CHECK (default_api_key_policy_id IS NULL OR length(default_api_key_policy_id) = 16)
        REFERENCES api_key_policies (id) ON DELETE RESTRICT,
    system_role TEXT UNIQUE
        CHECK (system_role IS NULL OR system_role IN ('user', 'admin')),
    filter_fast_mode INTEGER NOT NULL DEFAULT 0
        CHECK (filter_fast_mode IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

INSERT INTO user_groups (id, name, description, system_role)
VALUES
    (
        x'00000000000000000000000000000101',
        'Default Users',
        'Default group for newly invited users.',
        'user'
    ),
    (
        x'00000000000000000000000000000102',
        'Default Administrators',
        'Default group for newly invited administrators.',
        'admin'
    );

CREATE TABLE users (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    display_name TEXT NOT NULL UNIQUE CHECK (length(display_name) BETWEEN 1 AND 200),
    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('invited', 'active', 'suspended', 'disabled')),
    balance_amount TEXT NOT NULL DEFAULT '0'
        CHECK (
            length(balance_amount) > 0
            AND balance_amount NOT GLOB '*[^0-9.-]*'
            AND (
                balance_amount NOT GLOB '*-*'
                OR (
                    substr(balance_amount, 1, 1) = '-'
                    AND substr(balance_amount, 2) NOT GLOB '*-*'
                )
            )
            AND ltrim(balance_amount, '-') GLOB '[0-9]*'
            AND ltrim(balance_amount, '-') GLOB '*[0-9]'
            AND length(ltrim(balance_amount, '-'))
                - length(replace(ltrim(balance_amount, '-'), '.', '')) <= 1
            AND length(replace(ltrim(balance_amount, '-'), '.', '')) <= 24
            AND (
                CASE WHEN instr(ltrim(balance_amount, '-'), '.') = 0
                    THEN 0
                    ELSE length(ltrim(balance_amount, '-'))
                        - instr(ltrim(balance_amount, '-'), '.')
                END
            ) <= 8
            AND (
                CASE WHEN instr(ltrim(balance_amount, '-'), '.') = 0
                    THEN length(ltrim(balance_amount, '-'))
                    ELSE instr(ltrim(balance_amount, '-'), '.') - 1
                END
            ) <= 16
        ),
    email TEXT CHECK (email IS NULL OR length(email) <= 320),
    role TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'admin')),
    password_hash TEXT,
    auth_version INTEGER NOT NULL DEFAULT 1 CHECK (auth_version > 0),
    password_changed_at TEXT,
    default_api_key_policy_id BLOB CHECK (default_api_key_policy_id IS NULL OR length(default_api_key_policy_id) = 16)
        REFERENCES api_key_policies (id) ON DELETE RESTRICT,
    is_system INTEGER NOT NULL DEFAULT 0 CHECK (is_system IN (0, 1)),
    user_group_id BLOB NOT NULL CHECK (length(user_group_id) = 16)
        DEFAULT x'00000000000000000000000000000101'
        REFERENCES user_groups (id) ON DELETE RESTRICT,
    deleted_at TEXT,
    deleted_by BLOB CHECK (deleted_by IS NULL OR length(deleted_by) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    websocket_enabled INTEGER NOT NULL DEFAULT 0
        CHECK (websocket_enabled IN (0, 1)),
    password_change_required INTEGER NOT NULL DEFAULT 0
        CHECK (password_change_required IN (0, 1)),
    temporary_password_issued_at TEXT,
    temporary_password_expires_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (
        (
            password_change_required = 1
            AND password_hash IS NOT NULL
            AND temporary_password_issued_at IS NOT NULL
            AND temporary_password_expires_at IS NOT NULL
            AND temporary_password_expires_at > temporary_password_issued_at
        )
        OR (
            password_change_required = 0
            AND temporary_password_issued_at IS NULL
            AND temporary_password_expires_at IS NULL
        )
    )
);

CREATE TABLE user_sessions (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    user_id BLOB NOT NULL CHECK (length(user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    refresh_token_hash BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    revoked_at TEXT,
    rotated_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_seen_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    user_agent TEXT CHECK (user_agent IS NULL OR length(user_agent) <= 512),
    purpose TEXT NOT NULL DEFAULT 'normal'
        CHECK (purpose IN ('normal', 'password_change')),
    CHECK (expires_at > created_at)
);

CREATE TABLE user_invitations (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    user_id BLOB NOT NULL CHECK (length(user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    invited_by BLOB NOT NULL CHECK (length(invited_by) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    token_hash BLOB NOT NULL,
    expires_at TEXT NOT NULL,
    accepted_at TEXT,
    revoked_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (expires_at > created_at)
);

CREATE TABLE registration_invitation_codes (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    code_hash BLOB NOT NULL UNIQUE,
    max_uses INTEGER CHECK (max_uses IS NULL OR max_uses > 0),
    used_count INTEGER NOT NULL DEFAULT 0 CHECK (used_count >= 0),
    expires_at TEXT,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    user_group_id BLOB NOT NULL CHECK (length(user_group_id) = 16)
        REFERENCES user_groups (id) ON DELETE RESTRICT,
    initial_balance_amount TEXT NOT NULL DEFAULT '0'
        CHECK (
            length(initial_balance_amount) > 0
            AND initial_balance_amount NOT GLOB '*[^0-9.]*'
            AND initial_balance_amount GLOB '[0-9]*'
            AND initial_balance_amount GLOB '*[0-9]'
            AND length(initial_balance_amount)
                - length(replace(initial_balance_amount, '.', '')) <= 1
            AND length(replace(initial_balance_amount, '.', '')) <= 24
            AND (
                CASE WHEN instr(initial_balance_amount, '.') = 0
                    THEN 0
                    ELSE length(initial_balance_amount) - instr(initial_balance_amount, '.')
                END
            ) <= 8
            AND (
                CASE WHEN instr(initial_balance_amount, '.') = 0
                    THEN length(initial_balance_amount)
                    ELSE instr(initial_balance_amount, '.') - 1
                END
            ) <= 16
        ),
    created_by BLOB NOT NULL CHECK (length(created_by) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    last_used_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (max_uses IS NULL OR used_count <= max_uses)
);

CREATE TABLE models (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    source_model_id TEXT NOT NULL UNIQUE
        CHECK (length(source_model_id) BETWEEN 1 AND 300),
    display_name TEXT NOT NULL CHECK (length(display_name) BETWEEN 1 AND 300),
    provider_name TEXT CHECK (provider_name IS NULL OR length(provider_name) <= 200),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    currency TEXT NOT NULL DEFAULT 'USD' CHECK (currency = 'USD'),
    price_unit_tokens INTEGER NOT NULL CHECK (price_unit_tokens > 0),
    input_unit_price TEXT NOT NULL
        CHECK (
            length(input_unit_price) > 0
            AND input_unit_price NOT GLOB '*[^0-9.]*'
            AND input_unit_price GLOB '[0-9]*'
            AND input_unit_price GLOB '*[0-9]'
            AND length(input_unit_price) - length(replace(input_unit_price, '.', '')) <= 1
            AND length(replace(input_unit_price, '.', '')) <= 24
            AND (
                CASE WHEN instr(input_unit_price, '.') = 0
                    THEN 0
                    ELSE length(input_unit_price) - instr(input_unit_price, '.')
                END
            ) <= 12
            AND (
                CASE WHEN instr(input_unit_price, '.') = 0
                    THEN length(input_unit_price)
                    ELSE instr(input_unit_price, '.') - 1
                END
            ) <= 12
        ),
    cached_input_unit_price TEXT NOT NULL
        CHECK (
            length(cached_input_unit_price) > 0
            AND cached_input_unit_price NOT GLOB '*[^0-9.]*'
            AND cached_input_unit_price GLOB '[0-9]*'
            AND cached_input_unit_price GLOB '*[0-9]'
            AND length(cached_input_unit_price)
                - length(replace(cached_input_unit_price, '.', '')) <= 1
            AND length(replace(cached_input_unit_price, '.', '')) <= 24
            AND (
                CASE WHEN instr(cached_input_unit_price, '.') = 0
                    THEN 0
                    ELSE length(cached_input_unit_price) - instr(cached_input_unit_price, '.')
                END
            ) <= 12
            AND (
                CASE WHEN instr(cached_input_unit_price, '.') = 0
                    THEN length(cached_input_unit_price)
                    ELSE instr(cached_input_unit_price, '.') - 1
                END
            ) <= 12
        ),
    cache_write_unit_price TEXT NOT NULL
        CHECK (
            length(cache_write_unit_price) > 0
            AND cache_write_unit_price NOT GLOB '*[^0-9.]*'
            AND cache_write_unit_price GLOB '[0-9]*'
            AND cache_write_unit_price GLOB '*[0-9]'
            AND length(cache_write_unit_price)
                - length(replace(cache_write_unit_price, '.', '')) <= 1
            AND length(replace(cache_write_unit_price, '.', '')) <= 24
            AND (
                CASE WHEN instr(cache_write_unit_price, '.') = 0
                    THEN 0
                    ELSE length(cache_write_unit_price) - instr(cache_write_unit_price, '.')
                END
            ) <= 12
            AND (
                CASE WHEN instr(cache_write_unit_price, '.') = 0
                    THEN length(cache_write_unit_price)
                    ELSE instr(cache_write_unit_price, '.') - 1
                END
            ) <= 12
        ),
    output_unit_price TEXT NOT NULL
        CHECK (
            length(output_unit_price) > 0
            AND output_unit_price NOT GLOB '*[^0-9.]*'
            AND output_unit_price GLOB '[0-9]*'
            AND output_unit_price GLOB '*[0-9]'
            AND length(output_unit_price) - length(replace(output_unit_price, '.', '')) <= 1
            AND length(replace(output_unit_price, '.', '')) <= 24
            AND (
                CASE WHEN instr(output_unit_price, '.') = 0
                    THEN 0
                    ELSE length(output_unit_price) - instr(output_unit_price, '.')
                END
            ) <= 12
            AND (
                CASE WHEN instr(output_unit_price, '.') = 0
                    THEN length(output_unit_price)
                    ELSE instr(output_unit_price, '.') - 1
                END
            ) <= 12
        ),
    price_effective_at TEXT NOT NULL,
    source_payload TEXT NOT NULL DEFAULT '{}'
        CHECK (json_valid(source_payload) AND json_type(source_payload) = 'object'),
    last_synced_at TEXT,
    advanced_billing TEXT NOT NULL
        DEFAULT '{"long_context_tiers":[],"request_multipliers":[]}'
        CHECK (json_valid(advanced_billing) AND json_type(advanced_billing) = 'object'),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE connector_pools (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    connector_kind TEXT NOT NULL CHECK (connector_kind = 'codex_oauth'),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE channel_groups (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    api_format TEXT NOT NULL
        CHECK (
            api_format IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
        ),
    priority INTEGER NOT NULL CHECK (priority >= 0),
    selection_strategy TEXT NOT NULL
        CHECK (selection_strategy IN ('weighted_random', 'weighted_round_robin')),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    connector_kind TEXT NOT NULL DEFAULT 'openai_compatible'
        CHECK (connector_kind IN ('openai_compatible', 'codex_oauth')),
    connector_pool_id BLOB CHECK (connector_pool_id IS NULL OR length(connector_pool_id) = 16)
        REFERENCES connector_pools (id) ON DELETE RESTRICT,
    status_statistics_enabled INTEGER NOT NULL DEFAULT 0
        CHECK (status_statistics_enabled IN (0, 1)),
    request_compression TEXT NOT NULL DEFAULT 'default'
        CHECK (request_compression IN ('default', 'zstd')),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (id, api_format),
    CHECK (
        connector_kind <> 'codex_oauth'
        OR api_format IN ('open_ai_responses', 'open_ai_images')
    ),
    CHECK (
        (connector_kind = 'openai_compatible' AND connector_pool_id IS NULL)
        OR (connector_kind = 'codex_oauth' AND connector_pool_id IS NOT NULL)
    ),
    CHECK (request_compression = 'default' OR api_format = 'open_ai_responses')
);

CREATE TABLE proxies (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    proxy_url TEXT NOT NULL
        CHECK (
            lower(proxy_url) LIKE 'http://%'
            OR lower(proxy_url) LIKE 'https://%'
            OR lower(proxy_url) LIKE 'socks4://%'
            OR lower(proxy_url) LIKE 'socks4a://%'
            OR lower(proxy_url) LIKE 'socks5://%'
            OR lower(proxy_url) LIKE 'socks5h://%'
        ),
    username TEXT,
    password TEXT,
    no_proxy_hosts TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(no_proxy_hosts) AND json_type(no_proxy_hosts) = 'array'),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE config_templates (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    name TEXT NOT NULL UNIQUE CHECK (length(name) BETWEEN 1 AND 100),
    description TEXT,
    document TEXT NOT NULL
        CHECK (json_valid(document) AND json_type(document) = 'object'),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE channels (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    channel_group_id BLOB NOT NULL CHECK (length(channel_group_id) = 16),
    api_format TEXT NOT NULL
        CHECK (
            api_format IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
        ),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    base_url TEXT NOT NULL
        CHECK (lower(base_url) LIKE 'http://%' OR lower(base_url) LIKE 'https://%'),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    auto_disabled INTEGER NOT NULL DEFAULT 0 CHECK (auto_disabled IN (0, 1)),
    auto_disabled_reason TEXT
        CHECK (auto_disabled_reason IS NULL OR length(auto_disabled_reason) <= 500),
    weight INTEGER NOT NULL CHECK (weight > 0),
    proxy_id BLOB CHECK (proxy_id IS NULL OR length(proxy_id) = 16) REFERENCES proxies (id) ON DELETE RESTRICT,
    config_template_id BLOB CHECK (config_template_id IS NULL OR length(config_template_id) = 16) REFERENCES config_templates (id) ON DELETE RESTRICT,
    override_document TEXT NOT NULL DEFAULT '{}'
        CHECK (json_valid(override_document) AND json_type(override_document) = 'object'),
    connect_timeout_ms INTEGER
        CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms > 0),
    response_header_timeout_ms INTEGER
        CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms > 0),
    stream_idle_timeout_ms INTEGER
        CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms > 0),
    upstream_auth_kind TEXT NOT NULL
        CHECK (upstream_auth_kind IN ('none', 'bearer', 'header')),
    upstream_auth_header_name TEXT
        CHECK (upstream_auth_header_name IS NULL OR length(upstream_auth_header_name) <= 100),
    upstream_api_key TEXT,
    available_models TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(available_models) AND json_type(available_models) = 'array'),
    auto_disable_allowed INTEGER NOT NULL DEFAULT 0
        CHECK (auto_disable_allowed IN (0, 1)),
    test_model TEXT CHECK (test_model IS NULL OR length(test_model) <= 300),
    billing_multiplier TEXT NOT NULL DEFAULT '1'
        CHECK (
            length(billing_multiplier) > 0
            AND billing_multiplier NOT GLOB '*[^0-9.]*'
            AND billing_multiplier GLOB '[0-9]*'
            AND billing_multiplier GLOB '*[0-9]'
            AND length(billing_multiplier)
                - length(replace(billing_multiplier, '.', '')) <= 1
            AND length(replace(billing_multiplier, '.', '')) <= 24
            AND (
                CASE WHEN instr(billing_multiplier, '.') = 0
                    THEN 0
                    ELSE length(billing_multiplier) - instr(billing_multiplier, '.')
                END
            ) <= 12
            AND (
                CASE WHEN instr(billing_multiplier, '.') = 0
                    THEN length(billing_multiplier)
                    ELSE instr(billing_multiplier, '.') - 1
                END
            ) <= 12
        ),
    supports_websocket INTEGER NOT NULL DEFAULT 0
        CHECK (supports_websocket IN (0, 1)),
    supports_standalone_web_search INTEGER NOT NULL DEFAULT 0
        CHECK (supports_standalone_web_search IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (channel_group_id, name),
    UNIQUE (id, api_format),
    FOREIGN KEY (channel_group_id, api_format)
        REFERENCES channel_groups (id, api_format) ON DELETE RESTRICT,
    CHECK (auto_disabled = 1 OR auto_disabled_reason IS NULL),
    CHECK (
        (
            upstream_auth_kind = 'none'
            AND upstream_auth_header_name IS NULL
            AND upstream_api_key IS NULL
        )
        OR (
            upstream_auth_kind = 'bearer'
            AND upstream_auth_header_name IS NULL
            AND upstream_api_key IS NOT NULL
        )
        OR (
            upstream_auth_kind = 'header'
            AND upstream_auth_header_name IS NOT NULL
            AND upstream_api_key IS NOT NULL
        )
    ),
    CHECK (
        upstream_auth_header_name IS NULL
        OR lower(upstream_auth_header_name) NOT IN (
            'host',
            'content-length',
            'connection',
            'transfer-encoding',
            'authorization',
            'proxy-authorization',
            'proxy-authenticate',
            'keep-alive',
            'te',
            'trailer',
            'upgrade',
            'proxy-connection'
        )
    ),
    CHECK (supports_websocket = 0 OR api_format = 'open_ai_responses'),
    CHECK (
        supports_standalone_web_search = 0
        OR api_format = 'open_ai_responses'
    )
);

CREATE TABLE model_rules (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    client_model TEXT NOT NULL CHECK (length(client_model) BETWEEN 1 AND 300),
    api_format TEXT NOT NULL
        CHECK (
            api_format IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
        ),
    upstream_model_id BLOB NOT NULL CHECK (length(upstream_model_id) = 16) REFERENCES models (id) ON DELETE RESTRICT,
    channel_group_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(channel_group_ids) AND json_type(channel_group_ids) = 'array'),
    channel_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(channel_ids) AND json_type(channel_ids) = 'array'),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    description TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (client_model, api_format),
    CHECK (
        json_array_length(channel_group_ids) + json_array_length(channel_ids) > 0
    )
);

CREATE TABLE api_keys (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    user_id BLOB NOT NULL CHECK (length(user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    secret_value TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL
        CHECK (status IN ('active', 'disabled', 'revoked', 'expired')),
    expires_at TEXT,
    allowed_api_formats TEXT NOT NULL
        CHECK (
            json_valid(allowed_api_formats)
            AND json_type(allowed_api_formats) = 'array'
            AND json_array_length(allowed_api_formats) > 0
        ),
    permissions TEXT NOT NULL
        CHECK (
            json_valid(permissions)
            AND json_type(permissions) = 'array'
            AND json_array_length(permissions) > 0
        ),
    allowed_group_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(allowed_group_ids) AND json_type(allowed_group_ids) = 'array'),
    allowed_channel_ids TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(allowed_channel_ids) AND json_type(allowed_channel_ids) = 'array'),
    requests_per_minute INTEGER
        CHECK (requests_per_minute IS NULL OR requests_per_minute > 0),
    max_concurrent_requests INTEGER
        CHECK (max_concurrent_requests IS NULL OR max_concurrent_requests > 0),
    quota_limit_amount TEXT
        CHECK (
            quota_limit_amount IS NULL
            OR (
                length(quota_limit_amount) > 0
                AND quota_limit_amount NOT GLOB '*[^0-9.]*'
                AND quota_limit_amount GLOB '[0-9]*'
                AND quota_limit_amount GLOB '*[0-9]'
                AND length(quota_limit_amount)
                    - length(replace(quota_limit_amount, '.', '')) <= 1
                AND length(replace(quota_limit_amount, '.', '')) <= 24
                AND (
                    CASE WHEN instr(quota_limit_amount, '.') = 0
                        THEN 0
                        ELSE length(quota_limit_amount) - instr(quota_limit_amount, '.')
                    END
                ) <= 8
                AND (
                    CASE WHEN instr(quota_limit_amount, '.') = 0
                        THEN length(quota_limit_amount)
                        ELSE instr(quota_limit_amount, '.') - 1
                    END
                ) <= 16
            )
        ),
    quota_used_amount TEXT NOT NULL DEFAULT '0'
        CHECK (
            length(quota_used_amount) > 0
            AND quota_used_amount NOT GLOB '*[^0-9.]*'
            AND quota_used_amount GLOB '[0-9]*'
            AND quota_used_amount GLOB '*[0-9]'
            AND length(quota_used_amount)
                - length(replace(quota_used_amount, '.', '')) <= 1
            AND length(replace(quota_used_amount, '.', '')) <= 24
            AND (
                CASE WHEN instr(quota_used_amount, '.') = 0
                    THEN 0
                    ELSE length(quota_used_amount) - instr(quota_used_amount, '.')
                END
            ) <= 8
            AND (
                CASE WHEN instr(quota_used_amount, '.') = 0
                    THEN length(quota_used_amount)
                    ELSE instr(quota_used_amount, '.') - 1
                END
            ) <= 16
        ),
    last_used_at TEXT,
    is_system INTEGER NOT NULL DEFAULT 0 CHECK (is_system IN (0, 1)),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (user_id, name),
    CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE TABLE system_settings (
    setting_key TEXT NOT NULL PRIMARY KEY CHECK (length(setting_key) BETWEEN 1 AND 100),
    value TEXT NOT NULL
        CHECK (json_valid(value) AND json_type(value) = 'object'),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE mcp_servers (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE
        CHECK (
            length(slug) BETWEEN 1 AND 63
            AND substr(slug, 1, 1) GLOB '[a-z0-9]'
            AND slug NOT GLOB '*[^a-z0-9-]*'
        ),
    kind TEXT NOT NULL CHECK (kind IN ('web_search', 'image')),
    name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    description TEXT CHECK (description IS NULL OR length(description) <= 1000),
    model_rule_id BLOB NOT NULL CHECK (length(model_rule_id) = 16) REFERENCES model_rules (id) ON DELETE RESTRICT,
    settings_version INTEGER NOT NULL DEFAULT 1 CHECK (settings_version > 0),
    settings TEXT NOT NULL DEFAULT '{}'
        CHECK (json_valid(settings) AND json_type(settings) = 'object'),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    deleted_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (deleted_at IS NULL OR enabled = 0)
);

CREATE TABLE codex_oauth_credentials (
    channel_id BLOB NOT NULL CHECK (length(channel_id) = 16) PRIMARY KEY REFERENCES channels (id) ON DELETE RESTRICT,
    channel_group_id BLOB NOT NULL CHECK (length(channel_group_id) = 16)
        REFERENCES channel_groups (id) ON DELETE RESTRICT,
    connector_pool_id BLOB NOT NULL CHECK (length(connector_pool_id) = 16)
        REFERENCES connector_pools (id) ON DELETE RESTRICT,
    label TEXT NOT NULL
        CHECK (length(label) BETWEEN 1 AND 100 AND trim(label) <> ''),
    email TEXT CHECK (email IS NULL OR length(email) <= 320),
    account_id TEXT
        CHECK (account_id IS NULL OR (length(account_id) <= 300 AND trim(account_id) <> '')),
    user_id TEXT
        CHECK (user_id IS NULL OR (length(user_id) <= 300 AND trim(user_id) <> '')),
    plan_type TEXT CHECK (plan_type IS NULL OR length(plan_type) <= 100),
    is_fedramp INTEGER NOT NULL DEFAULT 0 CHECK (is_fedramp IN (0, 1)),
    id_token TEXT NOT NULL CHECK (length(id_token) > 0),
    access_token TEXT NOT NULL CHECK (length(access_token) > 0),
    refresh_token TEXT NOT NULL CHECK (length(refresh_token) > 0),
    access_token_expires_at TEXT,
    last_refreshed_at TEXT NOT NULL,
    refresh_generation INTEGER NOT NULL DEFAULT 0 CHECK (refresh_generation >= 0),
    reauth_required INTEGER NOT NULL DEFAULT 0 CHECK (reauth_required IN (0, 1)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    quota_threshold_percent INTEGER NOT NULL DEFAULT 95
        CHECK (quota_threshold_percent BETWEEN 1 AND 100),
    runtime_status TEXT NOT NULL DEFAULT 'active'
        CHECK (runtime_status IN ('active', 'draining', 'unavailable', 'disabled')),
    quota_allowed INTEGER CHECK (quota_allowed IS NULL OR quota_allowed IN (0, 1)),
    quota_limit_reached INTEGER
        CHECK (quota_limit_reached IS NULL OR quota_limit_reached IN (0, 1)),
    primary_used_percent INTEGER
        CHECK (primary_used_percent IS NULL OR primary_used_percent BETWEEN 0 AND 100),
    primary_window_seconds INTEGER
        CHECK (primary_window_seconds IS NULL OR primary_window_seconds > 0),
    primary_reset_at TEXT,
    secondary_used_percent INTEGER
        CHECK (secondary_used_percent IS NULL OR secondary_used_percent BETWEEN 0 AND 100),
    secondary_window_seconds INTEGER
        CHECK (secondary_window_seconds IS NULL OR secondary_window_seconds > 0),
    secondary_reset_at TEXT,
    quota_checked_at TEXT,
    quota_reset_credits_available INTEGER
        CHECK (
            quota_reset_credits_available IS NULL
            OR quota_reset_credits_available >= 0
        ),
    last_error_code TEXT
        CHECK (last_error_code IS NULL OR length(last_error_code) <= 100),
    last_error_summary TEXT
        CHECK (last_error_summary IS NULL OR length(last_error_summary) <= 1000),
    deleted_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (account_id IS NOT NULL OR user_id IS NOT NULL),
    CHECK (
        reauth_required = 0
        OR runtime_status IN ('unavailable', 'disabled')
    )
);

CREATE TABLE codex_oauth_credential_channels (
    credential_id BLOB NOT NULL CHECK (length(credential_id) = 16)
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    api_format TEXT NOT NULL
        CHECK (api_format IN ('open_ai_responses', 'open_ai_images')),
    channel_id BLOB NOT NULL CHECK (length(channel_id) = 16) UNIQUE,
    PRIMARY KEY (credential_id, api_format),
    FOREIGN KEY (channel_id, api_format)
        REFERENCES channels (id, api_format) ON DELETE RESTRICT
);

CREATE TABLE codex_oauth_flows (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    actor_user_id BLOB NOT NULL CHECK (length(actor_user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    channel_group_id BLOB NOT NULL CHECK (length(channel_group_id) = 16)
        REFERENCES channel_groups (id) ON DELETE RESTRICT,
    label TEXT NOT NULL
        CHECK (length(label) BETWEEN 1 AND 100 AND trim(label) <> ''),
    proxy_id BLOB CHECK (proxy_id IS NULL OR length(proxy_id) = 16) REFERENCES proxies (id) ON DELETE RESTRICT,
    weight INTEGER NOT NULL CHECK (weight > 0),
    quota_threshold_percent INTEGER NOT NULL
        CHECK (quota_threshold_percent BETWEEN 1 AND 100),
    redirect_uri TEXT NOT NULL
        CHECK (redirect_uri = 'http://localhost:1455/auth/callback'),
    state_hash BLOB NOT NULL CHECK (length(state_hash) = 32),
    code_verifier TEXT NOT NULL CHECK (length(code_verifier) BETWEEN 43 AND 128),
    expires_at TEXT NOT NULL,
    completed_at TEXT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE codex_quota_window_periods (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    credential_id BLOB NOT NULL CHECK (length(credential_id) = 16)
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    window_kind TEXT NOT NULL CHECK (window_kind IN ('primary', 'secondary')),
    window_seconds INTEGER NOT NULL CHECK (window_seconds > 0),
    started_at TEXT NOT NULL,
    scheduled_reset_at TEXT NOT NULL,
    ended_at TEXT,
    reset_reason TEXT
        CHECK (
            reset_reason IS NULL
            OR reset_reason IN ('natural', 'manual', 'openai_official')
        ),
    initial_used_percent INTEGER NOT NULL
        CHECK (initial_used_percent BETWEEN 0 AND 100),
    last_used_percent INTEGER NOT NULL
        CHECK (last_used_percent BETWEEN 0 AND 100),
    first_observed_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    CHECK (scheduled_reset_at > started_at),
    CHECK (ended_at IS NULL OR ended_at >= started_at),
    CHECK (
        (ended_at IS NULL AND reset_reason IS NULL)
        OR (ended_at IS NOT NULL AND reset_reason IS NOT NULL)
    ),
    CHECK (last_observed_at >= first_observed_at)
);

CREATE TABLE codex_quota_reset_events (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    credential_id BLOB NOT NULL CHECK (length(credential_id) = 16)
        REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT,
    actor_user_id BLOB NOT NULL CHECK (length(actor_user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    requested_at TEXT NOT NULL,
    outcome TEXT NOT NULL
        CHECK (
            outcome IN (
                'reset',
                'nothing_to_reset',
                'no_credit',
                'already_redeemed'
            )
        ),
    windows_reset INTEGER NOT NULL CHECK (windows_reset BETWEEN 0 AND 2),
    primary_applied_at TEXT,
    secondary_applied_at TEXT,
    correlation_id BLOB NOT NULL CHECK (length(correlation_id) = 16),
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE user_group_codex_quota_visibility (
    user_group_id BLOB NOT NULL CHECK (length(user_group_id) = 16)
        REFERENCES user_groups (id) ON DELETE CASCADE,
    channel_group_id BLOB NOT NULL CHECK (length(channel_group_id) = 16)
        REFERENCES channel_groups (id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (user_group_id, channel_group_id)
);

CREATE TABLE request_log_ingest (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    request_log_id BLOB NOT NULL CHECK (length(request_log_id) = 16),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    payload BLOB NOT NULL,
    staged_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_error_code TEXT
        CHECK (last_error_code IS NULL OR length(last_error_code) <= 100)
);

CREATE TABLE request_logs (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    started_at TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    user_id BLOB NOT NULL CHECK (length(user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    api_key_id BLOB NOT NULL CHECK (length(api_key_id) = 16) REFERENCES api_keys (id) ON DELETE RESTRICT,
    api_format TEXT NOT NULL
        CHECK (
            api_format IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
        ),
    api_operation TEXT NOT NULL,
    client_model TEXT NOT NULL CHECK (length(client_model) BETWEEN 1 AND 300),
    upstream_model TEXT
        CHECK (upstream_model IS NULL OR length(upstream_model) <= 300),
    model_rule_id BLOB CHECK (model_rule_id IS NULL OR length(model_rule_id) = 16) REFERENCES model_rules (id) ON DELETE RESTRICT,
    channel_group_id BLOB CHECK (channel_group_id IS NULL OR length(channel_group_id) = 16) REFERENCES channel_groups (id) ON DELETE RESTRICT,
    channel_id BLOB CHECK (channel_id IS NULL OR length(channel_id) = 16) REFERENCES channels (id) ON DELETE RESTRICT,
    outcome TEXT NOT NULL
        CHECK (outcome IN ('succeeded', 'failed', 'rejected', 'cancelled')),
    response_status_code INTEGER
        CHECK (
            response_status_code IS NULL
            OR response_status_code BETWEEN 100 AND 599
        ),
    streamed INTEGER NOT NULL DEFAULT 0 CHECK (streamed IN (0, 1)),
    ttft_ms INTEGER CHECK (ttft_ms IS NULL OR ttft_ms >= 0),
    total_duration_ms INTEGER
        CHECK (total_duration_ms IS NULL OR total_duration_ms >= 0),
    output_tokens_per_second TEXT
        CHECK (
            output_tokens_per_second IS NULL
            OR (
                length(output_tokens_per_second) > 0
                AND output_tokens_per_second NOT GLOB '*[^0-9.]*'
                AND output_tokens_per_second GLOB '[0-9]*'
                AND output_tokens_per_second GLOB '*[0-9]'
                AND length(output_tokens_per_second)
                    - length(replace(output_tokens_per_second, '.', '')) <= 1
                AND length(replace(output_tokens_per_second, '.', '')) <= 14
                AND (
                    CASE WHEN instr(output_tokens_per_second, '.') = 0
                        THEN 0
                        ELSE length(output_tokens_per_second) - instr(output_tokens_per_second, '.')
                    END
                ) <= 4
                AND (
                    CASE WHEN instr(output_tokens_per_second, '.') = 0
                        THEN length(output_tokens_per_second)
                        ELSE instr(output_tokens_per_second, '.') - 1
                    END
                ) <= 10
            )
        ),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    cached_input_tokens INTEGER
        CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
    cache_write_tokens INTEGER
        CHECK (cache_write_tokens IS NULL OR cache_write_tokens >= 0),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    reasoning_tokens INTEGER
        CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
    reasoning_effort TEXT
        CHECK (reasoning_effort IS NULL OR length(reasoning_effort) <= 32),
    fast_mode INTEGER NOT NULL DEFAULT 0 CHECK (fast_mode IN (0, 1)),
    model_id BLOB CHECK (model_id IS NULL OR length(model_id) = 16) REFERENCES models (id) ON DELETE RESTRICT,
    currency TEXT CHECK (currency IS NULL OR currency = 'USD'),
    price_unit_tokens INTEGER
        CHECK (price_unit_tokens IS NULL OR price_unit_tokens > 0),
    price_effective_at TEXT,
    input_unit_price TEXT
        CHECK (
            input_unit_price IS NULL
            OR (
                length(input_unit_price) > 0
                AND input_unit_price NOT GLOB '*[^0-9.]*'
                AND input_unit_price GLOB '[0-9]*'
                AND input_unit_price GLOB '*[0-9]'
                AND length(input_unit_price)
                    - length(replace(input_unit_price, '.', '')) <= 1
                AND length(replace(input_unit_price, '.', '')) <= 24
                AND (
                    CASE WHEN instr(input_unit_price, '.') = 0
                        THEN 0
                        ELSE length(input_unit_price) - instr(input_unit_price, '.')
                    END
                ) <= 12
                AND (
                    CASE WHEN instr(input_unit_price, '.') = 0
                        THEN length(input_unit_price)
                        ELSE instr(input_unit_price, '.') - 1
                    END
                ) <= 12
            )
        ),
    cached_input_unit_price TEXT
        CHECK (
            cached_input_unit_price IS NULL
            OR (
                length(cached_input_unit_price) > 0
                AND cached_input_unit_price NOT GLOB '*[^0-9.]*'
                AND cached_input_unit_price GLOB '[0-9]*'
                AND cached_input_unit_price GLOB '*[0-9]'
                AND length(cached_input_unit_price)
                    - length(replace(cached_input_unit_price, '.', '')) <= 1
                AND length(replace(cached_input_unit_price, '.', '')) <= 24
                AND (
                    CASE WHEN instr(cached_input_unit_price, '.') = 0
                        THEN 0
                        ELSE length(cached_input_unit_price) - instr(cached_input_unit_price, '.')
                    END
                ) <= 12
                AND (
                    CASE WHEN instr(cached_input_unit_price, '.') = 0
                        THEN length(cached_input_unit_price)
                        ELSE instr(cached_input_unit_price, '.') - 1
                    END
                ) <= 12
            )
        ),
    cache_write_unit_price TEXT
        CHECK (
            cache_write_unit_price IS NULL
            OR (
                length(cache_write_unit_price) > 0
                AND cache_write_unit_price NOT GLOB '*[^0-9.]*'
                AND cache_write_unit_price GLOB '[0-9]*'
                AND cache_write_unit_price GLOB '*[0-9]'
                AND length(cache_write_unit_price)
                    - length(replace(cache_write_unit_price, '.', '')) <= 1
                AND length(replace(cache_write_unit_price, '.', '')) <= 24
                AND (
                    CASE WHEN instr(cache_write_unit_price, '.') = 0
                        THEN 0
                        ELSE length(cache_write_unit_price) - instr(cache_write_unit_price, '.')
                    END
                ) <= 12
                AND (
                    CASE WHEN instr(cache_write_unit_price, '.') = 0
                        THEN length(cache_write_unit_price)
                        ELSE instr(cache_write_unit_price, '.') - 1
                    END
                ) <= 12
            )
        ),
    output_unit_price TEXT
        CHECK (
            output_unit_price IS NULL
            OR (
                length(output_unit_price) > 0
                AND output_unit_price NOT GLOB '*[^0-9.]*'
                AND output_unit_price GLOB '[0-9]*'
                AND output_unit_price GLOB '*[0-9]'
                AND length(output_unit_price)
                    - length(replace(output_unit_price, '.', '')) <= 1
                AND length(replace(output_unit_price, '.', '')) <= 24
                AND (
                    CASE WHEN instr(output_unit_price, '.') = 0
                        THEN 0
                        ELSE length(output_unit_price) - instr(output_unit_price, '.')
                    END
                ) <= 12
                AND (
                    CASE WHEN instr(output_unit_price, '.') = 0
                        THEN length(output_unit_price)
                        ELSE instr(output_unit_price, '.') - 1
                    END
                ) <= 12
            )
        ),
    cost_amount TEXT
        CHECK (
            cost_amount IS NULL
            OR (
                length(cost_amount) > 0
                AND cost_amount NOT GLOB '*[^0-9.]*'
                AND cost_amount GLOB '[0-9]*'
                AND cost_amount GLOB '*[0-9]'
                AND length(cost_amount) - length(replace(cost_amount, '.', '')) <= 1
                AND length(replace(cost_amount, '.', '')) <= 24
                AND (
                    CASE WHEN instr(cost_amount, '.') = 0
                        THEN 0
                        ELSE length(cost_amount) - instr(cost_amount, '.')
                    END
                ) <= 8
                AND (
                    CASE WHEN instr(cost_amount, '.') = 0
                        THEN length(cost_amount)
                        ELSE instr(cost_amount, '.') - 1
                    END
                ) <= 16
            )
        ),
    attempts TEXT NOT NULL DEFAULT '[]'
        CHECK (json_valid(attempts) AND json_type(attempts) = 'array'),
    error_code TEXT CHECK (error_code IS NULL OR length(error_code) <= 100),
    error_summary TEXT
        CHECK (error_summary IS NULL OR length(error_summary) <= 16384),
    billed_at TEXT,
    request_source TEXT NOT NULL DEFAULT 'client'
        CHECK (request_source IN ('client', 'mcp', 'scheduled_test')),
    request_protocol TEXT NOT NULL DEFAULT 'non_stream'
        CHECK (request_protocol IN ('non_stream', 'sse', 'websocket')),
    CHECK (completed_at >= started_at),
    CHECK (
        cached_input_tokens IS NULL
        OR (input_tokens IS NOT NULL AND cached_input_tokens <= input_tokens)
    ),
    CHECK (
        cache_write_tokens IS NULL
        OR (input_tokens IS NOT NULL AND cache_write_tokens <= input_tokens)
    ),
    CHECK (
        reasoning_tokens IS NULL
        OR (output_tokens IS NOT NULL AND reasoning_tokens <= output_tokens)
    ),
    CHECK (
        (
            currency IS NULL
            AND price_unit_tokens IS NULL
            AND price_effective_at IS NULL
            AND input_unit_price IS NULL
            AND cached_input_unit_price IS NULL
            AND cache_write_unit_price IS NULL
            AND output_unit_price IS NULL
        )
        OR (
            currency IS NOT NULL
            AND price_unit_tokens IS NOT NULL
            AND price_effective_at IS NOT NULL
            AND input_unit_price IS NOT NULL
            AND cached_input_unit_price IS NOT NULL
            AND cache_write_unit_price IS NOT NULL
            AND output_unit_price IS NOT NULL
        )
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
    ),
    CHECK (
        (
            api_format = 'open_ai_chat_completions'
            AND api_operation = 'chat_completions'
        )
        OR (
            api_format = 'open_ai_responses'
            AND api_operation IN ('responses', 'standalone_web_search')
        )
        OR (
            api_format = 'open_ai_images'
            AND api_operation IN ('images_generation', 'images_edit')
        )
    )
);

CREATE TABLE audit_logs (
    id BLOB NOT NULL CHECK (length(id) = 16) PRIMARY KEY,
    occurred_at TEXT NOT NULL
        DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    actor_user_id BLOB CHECK (actor_user_id IS NULL OR length(actor_user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    actor_type TEXT NOT NULL CHECK (actor_type IN ('user', 'system')),
    actor_role TEXT CHECK (actor_role IS NULL OR actor_role IN ('user', 'admin')),
    action TEXT NOT NULL CHECK (length(action) BETWEEN 1 AND 100),
    object_type TEXT NOT NULL,
    object_id BLOB NOT NULL CHECK (length(object_id) = 16),
    before_redacted TEXT
        CHECK (
            before_redacted IS NULL
            OR (json_valid(before_redacted) AND json_type(before_redacted) = 'object')
        ),
    after_redacted TEXT
        CHECK (
            after_redacted IS NULL
            OR (json_valid(after_redacted) AND json_type(after_redacted) = 'object')
        ),
    correlation_id TEXT
        CHECK (correlation_id IS NULL OR length(correlation_id) <= 100),
    reason TEXT CHECK (reason IS NULL OR length(reason) <= 500),
    source_ip_prefix TEXT
);

CREATE TABLE spend_leaderboard_periods (
    period TEXT NOT NULL CHECK (period IN ('day', 'week', 'month')),
    period_start TEXT NOT NULL,
    period_end TEXT NOT NULL,
    refreshed_at TEXT NOT NULL,
    total_cost_amount TEXT NOT NULL
        CHECK (
            length(total_cost_amount) > 0
            AND total_cost_amount NOT GLOB '*[^0-9.]*'
            AND total_cost_amount GLOB '[0-9]*'
            AND total_cost_amount GLOB '*[0-9]'
            AND length(total_cost_amount)
                - length(replace(total_cost_amount, '.', '')) <= 1
            AND length(replace(total_cost_amount, '.', '')) <= 24
            AND (
                CASE WHEN instr(total_cost_amount, '.') = 0
                    THEN 0
                    ELSE length(total_cost_amount) - instr(total_cost_amount, '.')
                END
            ) <= 8
            AND (
                CASE WHEN instr(total_cost_amount, '.') = 0
                    THEN length(total_cost_amount)
                    ELSE instr(total_cost_amount, '.') - 1
                END
            ) <= 16
        ),
    PRIMARY KEY (period, period_start),
    CHECK (period_end > period_start)
);

CREATE TABLE spend_leaderboard_entries (
    period TEXT NOT NULL,
    period_start TEXT NOT NULL,
    user_id BLOB NOT NULL CHECK (length(user_id) = 16) REFERENCES users (id) ON DELETE RESTRICT,
    rank INTEGER NOT NULL CHECK (rank > 0),
    request_count INTEGER NOT NULL CHECK (request_count >= 0),
    priced_request_count INTEGER NOT NULL
        CHECK (priced_request_count >= 0 AND priced_request_count <= request_count),
    total_tokens INTEGER NOT NULL CHECK (total_tokens >= 0),
    cost_amount TEXT NOT NULL
        CHECK (
            length(cost_amount) > 0
            AND cost_amount NOT GLOB '*[^0-9.]*'
            AND cost_amount GLOB '[0-9]*'
            AND cost_amount GLOB '*[0-9]'
            AND length(cost_amount) - length(replace(cost_amount, '.', '')) <= 1
            AND length(replace(cost_amount, '.', '')) <= 24
            AND (
                CASE WHEN instr(cost_amount, '.') = 0
                    THEN 0
                    ELSE length(cost_amount) - instr(cost_amount, '.')
                END
            ) <= 8
            AND (
                CASE WHEN instr(cost_amount, '.') = 0
                    THEN length(cost_amount)
                    ELSE instr(cost_amount, '.') - 1
                END
            ) <= 16
        ),
    PRIMARY KEY (period, period_start, user_id),
    FOREIGN KEY (period, period_start)
        REFERENCES spend_leaderboard_periods (period, period_start) ON DELETE CASCADE
);

-- Collection membership that PostgreSQL enforces with array operators is
-- expressed as triggers because SQLite prohibits subqueries in CHECK clauses.

CREATE TRIGGER api_key_policies_validate_collections_insert
BEFORE INSERT ON api_key_policies
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid API key policy collection');
END;

CREATE TRIGGER api_key_policies_validate_collections_update
BEFORE UPDATE OF allowed_group_ids, allowed_channel_ids ON api_key_policies
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid API key policy collection');
END;

CREATE TRIGGER api_keys_validate_collections_insert
BEFORE INSERT ON api_keys
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1
        FROM json_each(NEW.allowed_api_formats)
        WHERE
            type <> 'text'
            OR value NOT IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
    )
    OR EXISTS (
        SELECT 1
        FROM json_each(NEW.permissions)
        WHERE type <> 'text' OR value NOT IN ('proxy', 'models.read')
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid API key collection');
END;

CREATE TRIGGER api_keys_validate_collections_update
BEFORE UPDATE OF allowed_api_formats, permissions ON api_keys
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1
        FROM json_each(NEW.allowed_api_formats)
        WHERE
            type <> 'text'
            OR value NOT IN (
                'open_ai_chat_completions',
                'open_ai_responses',
                'open_ai_images'
            )
    )
    OR EXISTS (
        SELECT 1
        FROM json_each(NEW.permissions)
        WHERE type <> 'text' OR value NOT IN ('proxy', 'models.read')
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.allowed_channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid API key collection');
END;

CREATE TRIGGER model_rules_validate_collections_insert
BEFORE INSERT ON model_rules
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1 FROM json_each(NEW.channel_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid model rule collection');
END;

CREATE TRIGGER model_rules_validate_collections_update
BEFORE UPDATE OF channel_group_ids, channel_ids ON model_rules
FOR EACH ROW
WHEN
    EXISTS (
        SELECT 1 FROM json_each(NEW.channel_group_ids) WHERE type <> 'text'
    )
    OR EXISTS (
        SELECT 1 FROM json_each(NEW.channel_ids) WHERE type <> 'text'
    )
BEGIN
    SELECT RAISE(ABORT, 'invalid model rule collection');
END;

CREATE TRIGGER proxies_validate_no_proxy_hosts_insert
BEFORE INSERT ON proxies
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM json_each(NEW.no_proxy_hosts) WHERE type <> 'text'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid no_proxy_hosts collection');
END;

CREATE TRIGGER proxies_validate_no_proxy_hosts_update
BEFORE UPDATE OF no_proxy_hosts ON proxies
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM json_each(NEW.no_proxy_hosts) WHERE type <> 'text'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid no_proxy_hosts collection');
END;

CREATE TRIGGER channels_validate_available_models_insert
BEFORE INSERT ON channels
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM json_each(NEW.available_models) WHERE type <> 'text'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid available_models collection');
END;

CREATE TRIGGER channels_validate_available_models_update
BEFORE UPDATE OF available_models ON channels
FOR EACH ROW
WHEN EXISTS (
    SELECT 1 FROM json_each(NEW.available_models) WHERE type <> 'text'
)
BEGIN
    SELECT RAISE(ABORT, 'invalid available_models collection');
END;

CREATE TRIGGER channels_validate_test_model_insert
BEFORE INSERT ON channels
FOR EACH ROW
WHEN
    NEW.test_model IS NOT NULL
    AND NOT EXISTS (
        SELECT 1
        FROM json_each(NEW.available_models)
        WHERE type = 'text' AND value = NEW.test_model
    )
BEGIN
    SELECT RAISE(ABORT, 'test_model must be in available_models');
END;

CREATE TRIGGER channels_validate_test_model_update
BEFORE UPDATE OF test_model, available_models ON channels
FOR EACH ROW
WHEN
    NEW.test_model IS NOT NULL
    AND NOT EXISTS (
        SELECT 1
        FROM json_each(NEW.available_models)
        WHERE type = 'text' AND value = NEW.test_model
    )
BEGIN
    SELECT RAISE(ABORT, 'test_model must be in available_models');
END;

-- SQLite cannot assign NEW.updated_at in a BEFORE trigger. These AFTER
-- triggers make the value strictly newer when callers do not set it.

CREATE TRIGGER api_key_policies_set_updated_at
AFTER UPDATE ON api_key_policies
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE api_key_policies
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER api_keys_set_updated_at
AFTER UPDATE ON api_keys
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE api_keys
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER channel_groups_set_updated_at
AFTER UPDATE ON channel_groups
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE channel_groups
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER channels_set_updated_at
AFTER UPDATE ON channels
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE channels
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER codex_oauth_credentials_set_updated_at
AFTER UPDATE ON codex_oauth_credentials
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE codex_oauth_credentials
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE channel_id = NEW.channel_id;
END;

CREATE TRIGGER codex_quota_window_periods_set_updated_at
AFTER UPDATE ON codex_quota_window_periods
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE codex_quota_window_periods
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER config_templates_set_updated_at
AFTER UPDATE ON config_templates
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE config_templates
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER mcp_servers_set_updated_at
AFTER UPDATE ON mcp_servers
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE mcp_servers
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER model_rules_set_updated_at
AFTER UPDATE ON model_rules
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE model_rules
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER models_set_updated_at
AFTER UPDATE ON models
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE models
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER proxies_set_updated_at
AFTER UPDATE ON proxies
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE proxies
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER registration_invitation_codes_set_updated_at
AFTER UPDATE ON registration_invitation_codes
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE registration_invitation_codes
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER system_settings_set_updated_at
AFTER UPDATE ON system_settings
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE system_settings
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE setting_key = NEW.setting_key;
END;

CREATE TRIGGER user_groups_set_updated_at
AFTER UPDATE ON user_groups
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE user_groups
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

CREATE TRIGGER users_set_updated_at
AFTER UPDATE ON users
FOR EACH ROW
WHEN NEW.updated_at = OLD.updated_at
BEGIN
    UPDATE users
    SET updated_at = CASE
        WHEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now') <= OLD.updated_at
            THEN strftime(
                '%Y-%m-%dT%H:%M:%fZ',
                julianday(OLD.updated_at) + 1.0 / 86400000.0
            )
        ELSE strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
    END
    WHERE id = NEW.id;
END;

-- The immutable log facts retain the PostgreSQL append-only boundary.

CREATE TRIGGER audit_logs_prevent_update
BEFORE UPDATE ON audit_logs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'audit_logs are append-only');
END;

CREATE TRIGGER audit_logs_prevent_delete
BEFORE DELETE ON audit_logs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'audit_logs are append-only');
END;

CREATE TRIGGER request_logs_prevent_fact_update
BEFORE UPDATE OF
    id,
    started_at,
    completed_at,
    user_id,
    api_key_id,
    api_format,
    api_operation,
    client_model,
    upstream_model,
    model_rule_id,
    channel_group_id,
    channel_id,
    outcome,
    response_status_code,
    streamed,
    ttft_ms,
    total_duration_ms,
    output_tokens_per_second,
    input_tokens,
    cached_input_tokens,
    cache_write_tokens,
    output_tokens,
    reasoning_tokens,
    reasoning_effort,
    fast_mode,
    model_id,
    currency,
    price_unit_tokens,
    price_effective_at,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    cost_amount,
    attempts,
    error_code,
    error_summary,
    request_source,
    request_protocol
ON request_logs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'request_logs facts are immutable');
END;

CREATE TRIGGER request_logs_validate_billing_update
BEFORE UPDATE OF billed_at ON request_logs
FOR EACH ROW
WHEN OLD.billed_at IS NOT NULL OR NEW.billed_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'request_logs may set billed_at only once');
END;

CREATE TRIGGER request_logs_prevent_delete
BEFORE DELETE ON request_logs
FOR EACH ROW
BEGIN
    SELECT RAISE(ABORT, 'request_logs are append-only');
END;

-- Query and projection indexes mirror the current PostgreSQL access paths.

CREATE INDEX api_keys_user_id_status_idx
    ON api_keys (user_id, status);
CREATE INDEX audit_logs_actor_occurred_at_idx
    ON audit_logs (actor_user_id, occurred_at DESC);
CREATE INDEX audit_logs_object_occurred_at_idx
    ON audit_logs (object_type, object_id, occurred_at DESC);
CREATE UNIQUE INDEX channel_groups_connector_pool_format_idx
    ON channel_groups (connector_pool_id, api_format)
    WHERE connector_pool_id IS NOT NULL;
CREATE INDEX channel_groups_status_statistics_enabled_idx
    ON channel_groups (id)
    WHERE status_statistics_enabled = 1;
CREATE INDEX channels_channel_group_id_enabled_idx
    ON channels (channel_group_id, enabled);
CREATE UNIQUE INDEX codex_oauth_credentials_pool_identity_idx
    ON codex_oauth_credentials (
        connector_pool_id,
        ifnull(account_id, ''),
        ifnull(user_id, '')
    )
    WHERE deleted_at IS NULL;
CREATE INDEX codex_oauth_credentials_quota_idx
    ON codex_oauth_credentials (runtime_status, quota_checked_at);
CREATE INDEX codex_oauth_credentials_refresh_idx
    ON codex_oauth_credentials (runtime_status, access_token_expires_at);
CREATE INDEX codex_oauth_flows_expiry_idx
    ON codex_oauth_flows (expires_at)
    WHERE completed_at IS NULL;
CREATE INDEX codex_quota_reset_events_pending_idx
    ON codex_quota_reset_events (credential_id, requested_at DESC)
    WHERE
        outcome IN ('reset', 'already_redeemed')
        AND windows_reset
            > (primary_applied_at IS NOT NULL) + (secondary_applied_at IS NOT NULL);
CREATE UNIQUE INDEX codex_quota_window_periods_current_idx
    ON codex_quota_window_periods (credential_id, window_kind)
    WHERE ended_at IS NULL;
CREATE INDEX codex_quota_window_periods_history_idx
    ON codex_quota_window_periods (credential_id, window_kind, started_at DESC);
CREATE UNIQUE INDEX codex_quota_window_periods_identity_idx
    ON codex_quota_window_periods (
        credential_id,
        window_kind,
        started_at,
        scheduled_reset_at
    );
CREATE INDEX mcp_servers_active_kind_idx
    ON mcp_servers (kind, slug)
    WHERE deleted_at IS NULL;
CREATE INDEX registration_invitation_codes_user_group_id_idx
    ON registration_invitation_codes (user_group_id);
CREATE INDEX request_log_ingest_retry_idx
    ON request_log_ingest (next_attempt_at, sequence)
    WHERE attempt_count > 0;
CREATE INDEX request_logs_api_format_started_at_idx
    ON request_logs (api_format, started_at DESC, id DESC);
CREATE INDEX request_logs_api_key_id_started_at_idx
    ON request_logs (api_key_id, started_at DESC);
CREATE INDEX request_logs_channel_group_model_started_at_idx
    ON request_logs (channel_group_id, upstream_model, started_at DESC)
    WHERE channel_group_id IS NOT NULL;
CREATE INDEX request_logs_channel_id_started_at_idx
    ON request_logs (channel_id, started_at DESC);
CREATE INDEX request_logs_channel_model_started_at_idx
    ON request_logs (channel_id, upstream_model, started_at DESC)
    WHERE channel_id IS NOT NULL;
CREATE INDEX request_logs_client_model_started_at_idx
    ON request_logs (client_model, started_at DESC, id DESC);
CREATE INDEX request_logs_failed_started_at_idx
    ON request_logs (started_at DESC)
    WHERE outcome = 'failed';
CREATE INDEX request_logs_mcp_started_at_idx
    ON request_logs (started_at DESC, id DESC)
    WHERE request_source = 'mcp';
CREATE INDEX request_logs_outcome_started_at_idx
    ON request_logs (outcome, started_at DESC, id DESC);
CREATE INDEX request_logs_scheduled_test_started_at_idx
    ON request_logs (started_at DESC, id DESC)
    WHERE request_source = 'scheduled_test';
CREATE INDEX request_logs_started_at_id_idx
    ON request_logs (started_at DESC, id DESC);
CREATE INDEX request_logs_unbilled_settlement_idx
    ON request_logs (completed_at, id)
    WHERE billed_at IS NULL AND cost_amount IS NOT NULL;
CREATE INDEX request_logs_upstream_model_started_at_idx
    ON request_logs (upstream_model, started_at DESC, id DESC);
CREATE INDEX request_logs_user_id_started_at_idx
    ON request_logs (user_id, started_at DESC);
CREATE INDEX spend_leaderboard_entries_period_rank_idx
    ON spend_leaderboard_entries (period, period_start, rank);
CREATE INDEX user_group_codex_quota_visibility_channel_group_idx
    ON user_group_codex_quota_visibility (channel_group_id);
CREATE INDEX user_groups_default_api_key_policy_id_idx
    ON user_groups (default_api_key_policy_id);
CREATE INDEX user_invitations_user_id_active_idx
    ON user_invitations (user_id, expires_at DESC)
    WHERE accepted_at IS NULL AND revoked_at IS NULL;
CREATE INDEX user_sessions_user_id_active_idx
    ON user_sessions (user_id, expires_at DESC)
    WHERE revoked_at IS NULL;
CREATE INDEX users_default_api_key_policy_id_idx
    ON users (default_api_key_policy_id);
CREATE INDEX users_deleted_at_idx
    ON users (deleted_at)
    WHERE deleted_at IS NOT NULL;
CREATE UNIQUE INDEX users_email_lower_unique_idx
    ON users (lower(email))
    WHERE email IS NOT NULL;
CREATE INDEX users_user_group_id_idx
    ON users (user_group_id)
    WHERE deleted_at IS NULL;
