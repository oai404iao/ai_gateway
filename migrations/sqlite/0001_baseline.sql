-- Current SQLite schema. Money is stored as validated decimal text.

CREATE TABLE _gateway_codex_operations (
    credential_id TEXT PRIMARY KEY REFERENCES codex_oauth_credentials(channel_id) ON DELETE RESTRICT,
    attempt_id TEXT NOT NULL UNIQUE CHECK(ag_uuid_valid(attempt_id)),
    kind TEXT NOT NULL CHECK(kind IN ('refresh','quota_reset')),
    generation INTEGER NOT NULL CHECK(generation>=0),
    started_at TEXT NOT NULL DEFAULT (ag_now()) CHECK(ag_time_valid(started_at))
) STRICT;

CREATE TABLE api_key_channel_grants (
    api_key_id TEXT NOT NULL CONSTRAINT api_key_channel_grants_api_key_id_storage CHECK (ag_uuid_valid(api_key_id) AND instr(api_key_id,char(0))=0),
    channel_id TEXT NOT NULL CONSTRAINT api_key_channel_grants_channel_id_storage CHECK (ag_uuid_valid(channel_id) AND instr(channel_id,char(0))=0),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CONSTRAINT api_key_channel_grants_created_at_storage CHECK (ag_time_valid(created_at) AND instr(created_at,char(0))=0),
    CONSTRAINT api_key_channel_grants_pkey PRIMARY KEY(api_key_id,channel_id),
    CONSTRAINT api_key_channel_grants_api_key_id_fkey FOREIGN KEY(api_key_id) REFERENCES api_keys(id) ON DELETE CASCADE,
    CONSTRAINT api_key_channel_grants_channel_id_fkey FOREIGN KEY(channel_id) REFERENCES upstream_channels(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE api_key_policies (
    id TEXT NOT NULL CONSTRAINT api_key_policies_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT api_key_policies_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    allowed_group_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_key_policies_allowed_group_ids_storage CHECK (allowed_group_ids IS NULL OR (ag_array_valid(allowed_group_ids, 'uuid'))) CHECK (allowed_group_ids IS NULL OR instr(allowed_group_ids, char(0))=0),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT api_key_policies_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_key_policies_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_key_policies_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    allowed_channel_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_key_policies_allowed_channel_ids_storage CHECK (allowed_channel_ids IS NULL OR (ag_array_valid(allowed_channel_ids, 'uuid'))) CHECK (allowed_channel_ids IS NULL OR instr(allowed_channel_ids, char(0))=0),
    CONSTRAINT api_key_policies_allowed_channel_ids_no_nulls CHECK ((ag_array_position(allowed_channel_ids, NULL) IS NULL)),
    CONSTRAINT api_key_policies_allowed_group_ids_no_nulls CHECK ((ag_array_position(allowed_group_ids, NULL) IS NULL)),
    CONSTRAINT api_key_policies_name_key UNIQUE (name),
    CONSTRAINT api_key_policies_pkey PRIMARY KEY (id)
) STRICT;

CREATE TABLE api_key_policy_channel_grants (
    policy_id TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_policy_id_storage CHECK (ag_uuid_valid(policy_id) AND instr(policy_id,char(0))=0),
    channel_id TEXT NOT NULL CONSTRAINT api_key_policy_channel_grants_channel_id_storage CHECK (ag_uuid_valid(channel_id) AND instr(channel_id,char(0))=0),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CONSTRAINT api_key_policy_channel_grants_created_at_storage CHECK (ag_time_valid(created_at) AND instr(created_at,char(0))=0),
    CONSTRAINT api_key_policy_channel_grants_pkey PRIMARY KEY(policy_id,channel_id),
    CONSTRAINT api_key_policy_channel_grants_policy_id_fkey FOREIGN KEY(policy_id) REFERENCES api_key_policies(id) ON DELETE CASCADE,
    CONSTRAINT api_key_policy_channel_grants_channel_id_fkey FOREIGN KEY(channel_id) REFERENCES upstream_channels(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE "api_keys" (
    id TEXT NOT NULL CONSTRAINT api_keys_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT api_keys_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT api_keys_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    secret_value TEXT NOT NULL CHECK (secret_value IS NULL OR instr(secret_value, char(0))=0),
    status TEXT NOT NULL CHECK (status IS NULL OR instr(status, char(0))=0),
    expires_at TEXT CONSTRAINT api_keys_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    permissions TEXT NOT NULL CONSTRAINT api_keys_permissions_storage CHECK (permissions IS NULL OR (ag_array_valid(permissions, 'text'))) CHECK (permissions IS NULL OR instr(permissions, char(0))=0),
    allowed_group_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_keys_allowed_group_ids_storage CHECK (allowed_group_ids IS NULL OR (ag_array_valid(allowed_group_ids, 'uuid'))) CHECK (allowed_group_ids IS NULL OR instr(allowed_group_ids, char(0))=0),
    requests_per_minute INTEGER CONSTRAINT api_keys_requests_per_minute_storage CHECK (requests_per_minute IS NULL OR (requests_per_minute BETWEEN -2147483648 AND 2147483647)),
    max_concurrent_requests INTEGER CONSTRAINT api_keys_max_concurrent_requests_storage CHECK (max_concurrent_requests IS NULL OR (max_concurrent_requests BETWEEN -2147483648 AND 2147483647)),
    quota_limit_amount TEXT CONSTRAINT api_keys_quota_limit_amount_storage CHECK (quota_limit_amount IS NULL OR (ag_decimal_valid(quota_limit_amount, 24, 8))) CHECK (quota_limit_amount IS NULL OR instr(quota_limit_amount, char(0))=0),
    quota_used_amount TEXT DEFAULT ('0') NOT NULL CONSTRAINT api_keys_quota_used_amount_storage CHECK (quota_used_amount IS NULL OR (ag_decimal_valid(quota_used_amount, 24, 8))) CHECK (quota_used_amount IS NULL OR instr(quota_used_amount, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_keys_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT api_keys_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    allowed_channel_ids TEXT DEFAULT ('[]') NOT NULL CONSTRAINT api_keys_allowed_channel_ids_storage CHECK (allowed_channel_ids IS NULL OR (ag_array_valid(allowed_channel_ids, 'uuid'))) CHECK (allowed_channel_ids IS NULL OR instr(allowed_channel_ids, char(0))=0),
    is_system INTEGER DEFAULT (0) NOT NULL CONSTRAINT api_keys_is_system_storage CHECK (is_system IS NULL OR (is_system IN (0,1))),
    deleted_at TEXT CONSTRAINT api_keys_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    deleted_by TEXT CONSTRAINT api_keys_deleted_by_storage CHECK (deleted_by IS NULL OR (ag_uuid_valid(deleted_by))) CHECK (deleted_by IS NULL OR instr(deleted_by, char(0))=0),
    CONSTRAINT api_keys_allowed_channel_ids_no_nulls CHECK ((ag_array_position(allowed_channel_ids, NULL) IS NULL)),
    CONSTRAINT api_keys_allowed_group_ids_no_nulls CHECK ((ag_array_position(allowed_group_ids, NULL) IS NULL)),
    CONSTRAINT api_keys_check CHECK (((expires_at IS NULL) OR (expires_at > created_at))),
    CONSTRAINT api_keys_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT api_keys_deleted_state_check CHECK (((deleted_at IS NULL) OR (status = 'revoked'))),
    CONSTRAINT api_keys_max_concurrent_requests_check CHECK (((max_concurrent_requests IS NULL) OR (max_concurrent_requests > 0))),
    CONSTRAINT api_keys_permissions_check CHECK (ag_array_subset(permissions, json_array('proxy', 'models.read'))),
    CONSTRAINT api_keys_permissions_check1 CHECK ((ag_array_position(permissions, NULL) IS NULL)),
    CONSTRAINT api_keys_permissions_nonempty CHECK ((json_array_length(permissions) > 0)),
    CONSTRAINT api_keys_quota_limit_amount_check CHECK (((quota_limit_amount IS NULL) OR (ag_decimal_cmp(quota_limit_amount, '0') >= 0))),
    CONSTRAINT api_keys_quota_used_amount_check CHECK ((ag_decimal_cmp(quota_used_amount, '0') >= 0)),
    CONSTRAINT api_keys_requests_per_minute_check CHECK (((requests_per_minute IS NULL) OR (requests_per_minute > 0))),
    CONSTRAINT api_keys_status_check CHECK (ag_array_contains(json_array('active', 'disabled', 'revoked', 'expired'), status)),
    CONSTRAINT api_keys_pkey PRIMARY KEY (id),
    CONSTRAINT api_keys_secret_value_key UNIQUE (secret_value),
    CONSTRAINT api_keys_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE audit_logs (
    id TEXT NOT NULL CONSTRAINT audit_logs_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    occurred_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT audit_logs_occurred_at_storage CHECK (occurred_at IS NULL OR (ag_time_valid(occurred_at))) CHECK (occurred_at IS NULL OR instr(occurred_at, char(0))=0),
    actor_user_id TEXT CONSTRAINT audit_logs_actor_user_id_storage CHECK (actor_user_id IS NULL OR (ag_uuid_valid(actor_user_id))) CHECK (actor_user_id IS NULL OR instr(actor_user_id, char(0))=0),
    actor_type TEXT NOT NULL CHECK (actor_type IS NULL OR instr(actor_type, char(0))=0),
    action TEXT NOT NULL CONSTRAINT audit_logs_action_storage CHECK (action IS NULL OR (length(action) <= 100)) CHECK (action IS NULL OR instr(action, char(0))=0),
    object_type TEXT NOT NULL CHECK (object_type IS NULL OR instr(object_type, char(0))=0),
    object_id TEXT NOT NULL CONSTRAINT audit_logs_object_id_storage CHECK (object_id IS NULL OR (ag_uuid_valid(object_id))) CHECK (object_id IS NULL OR instr(object_id, char(0))=0),
    before_redacted TEXT CONSTRAINT audit_logs_before_redacted_storage CHECK (before_redacted IS NULL OR (ag_json_valid(before_redacted))) CHECK (before_redacted IS NULL OR instr(before_redacted, char(0))=0),
    after_redacted TEXT CONSTRAINT audit_logs_after_redacted_storage CHECK (after_redacted IS NULL OR (ag_json_valid(after_redacted))) CHECK (after_redacted IS NULL OR instr(after_redacted, char(0))=0),
    correlation_id TEXT CONSTRAINT audit_logs_correlation_id_storage CHECK (correlation_id IS NULL OR (length(correlation_id) <= 100)) CHECK (correlation_id IS NULL OR instr(correlation_id, char(0))=0),
    reason TEXT CONSTRAINT audit_logs_reason_storage CHECK (reason IS NULL OR (length(reason) <= 500)) CHECK (reason IS NULL OR instr(reason, char(0))=0),
    actor_role TEXT CHECK (actor_role IS NULL OR instr(actor_role, char(0))=0),
    CONSTRAINT audit_logs_actor_role_check CHECK (((actor_role IS NULL) OR ag_array_contains(json_array('user', 'admin'), actor_role))),
    CONSTRAINT audit_logs_actor_type_check CHECK (ag_array_contains(json_array('user', 'system'), actor_type)),
    CONSTRAINT audit_logs_after_redacted_check CHECK (((after_redacted IS NULL) OR (json_type(after_redacted) = 'object'))),
    CONSTRAINT audit_logs_before_redacted_check CHECK (((before_redacted IS NULL) OR (json_type(before_redacted) = 'object'))),
    CONSTRAINT audit_logs_pkey PRIMARY KEY (id),
    CONSTRAINT audit_logs_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "channel_capabilities" (
    id TEXT NOT NULL CONSTRAINT channel_capabilities_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    channel_id TEXT NOT NULL CONSTRAINT channel_capabilities_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id) AND instr(channel_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT channel_capabilities_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    enabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    available_models TEXT DEFAULT '[]' NOT NULL CONSTRAINT channel_capabilities_available_models_storage CHECK (available_models IS NULL OR (ag_array_valid(available_models, 'text') AND instr(available_models, char(0))=0)),
    request_compression TEXT DEFAULT 'default' NOT NULL CONSTRAINT channel_capabilities_request_compression_storage CHECK (request_compression IS NULL OR instr(request_compression, char(0))=0),
    test_model TEXT CONSTRAINT channel_capabilities_test_model_storage CHECK (test_model IS NULL OR (length(test_model) <= 300 AND instr(test_model, char(0))=0)),
    test_pricing_model_id TEXT CONSTRAINT channel_capabilities_test_pricing_model_id_storage CHECK (test_pricing_model_id IS NULL OR (ag_uuid_valid(test_pricing_model_id) AND instr(test_pricing_model_id, char(0))=0)),
    auto_disabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_auto_disabled_storage CHECK (auto_disabled IS NULL OR auto_disabled IN (0,1)),
    auto_disable_reason TEXT CONSTRAINT channel_capabilities_auto_disable_reason_storage CHECK (auto_disable_reason IS NULL OR (length(auto_disable_reason) <= 500 AND instr(auto_disable_reason, char(0))=0)),
    auto_disable_at TEXT CONSTRAINT channel_capabilities_auto_disable_at_storage CHECK (auto_disable_at IS NULL OR (ag_time_valid(auto_disable_at) AND instr(auto_disable_at, char(0))=0)),
    auto_disable_allowed INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_auto_disable_allowed_storage CHECK (auto_disable_allowed IS NULL OR auto_disable_allowed IN (0,1)),
    status_statistics_enabled INTEGER DEFAULT 0 NOT NULL CONSTRAINT channel_capabilities_status_statistics_enabled_storage CHECK (status_statistics_enabled IS NULL OR status_statistics_enabled IN (0,1)),
    config_template_id TEXT CONSTRAINT channel_capabilities_config_template_id_storage CHECK (config_template_id IS NULL OR (ag_uuid_valid(config_template_id) AND instr(config_template_id, char(0))=0)),
    override_document TEXT DEFAULT '{}' NOT NULL CONSTRAINT channel_capabilities_override_document_storage CHECK (override_document IS NULL OR (ag_json_valid(override_document) AND instr(override_document, char(0))=0)),
    billing_multiplier TEXT DEFAULT '1' NOT NULL CONSTRAINT channel_capabilities_billing_multiplier_storage CHECK (billing_multiplier IS NULL OR (ag_decimal_valid(billing_multiplier, 24, 12) AND instr(billing_multiplier, char(0))=0)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT channel_capabilities_revision_storage CHECK (revision IS NULL OR (ag_uuid_valid(revision) AND instr(revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_capabilities_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_capabilities_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT channel_capabilities_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT channel_capabilities_channel_operation_key
        UNIQUE (channel_id, operation),
    CONSTRAINT channel_capabilities_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT channel_capabilities_operation_check CHECK (operation IN ('chat_completion','responses','responses-ws','web_search','images_edit','images_generation')),
    CONSTRAINT channel_capabilities_available_models_check
        CHECK (ag_array_position(available_models, NULL) IS NULL),
    CONSTRAINT channel_capabilities_request_compression_check
        CHECK (ag_array_contains(json_array('default', 'zstd'), request_compression)),
    CONSTRAINT channel_capabilities_request_compression_operation_check CHECK (request_compression='default' OR operation='responses'),
    CONSTRAINT channel_capabilities_override_document_check
        CHECK (json_type(override_document) = 'object'),
    CONSTRAINT channel_capabilities_billing_multiplier_check
        CHECK (ag_decimal_cmp(billing_multiplier, '0') >= 0),
    CONSTRAINT channel_capabilities_test_model_pair_check
        CHECK ((test_model IS NULL) = (test_pricing_model_id IS NULL)),
    CONSTRAINT channel_capabilities_test_model_catalogue_check
        CHECK (test_model IS NULL OR ag_array_contains(available_models, test_model)),
    CONSTRAINT channel_capabilities_probe_check CHECK (test_model IS NULL OR operation IN ('chat_completion','responses')),
    CONSTRAINT channel_capabilities_auto_disable_check
        CHECK (auto_disabled OR (auto_disable_reason IS NULL AND auto_disable_at IS NULL)),
    CONSTRAINT channel_capabilities_deleted_state_check
        CHECK (deleted_at IS NULL OR (NOT enabled AND NOT auto_disabled)),
    CONSTRAINT channel_capabilities_pkey PRIMARY KEY (id),
    CONSTRAINT channel_capabilities_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    CONSTRAINT channel_capabilities_test_pricing_model_id_fkey FOREIGN KEY (test_pricing_model_id) REFERENCES models (id) ON DELETE RESTRICT,
    CONSTRAINT channel_capabilities_config_template_id_fkey FOREIGN KEY (config_template_id) REFERENCES config_templates (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE channel_identity_registry (
    id TEXT NOT NULL CONSTRAINT channel_identity_registry_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    label TEXT NOT NULL CONSTRAINT channel_identity_registry_label_storage CHECK (label IS NULL OR (length(label) <= 100 AND instr(label, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT channel_identity_registry_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    canonical_channel_id TEXT CONSTRAINT channel_identity_registry_canonical_channel_id_storage CHECK (canonical_channel_id IS NULL OR (ag_uuid_valid(canonical_channel_id) AND instr(canonical_channel_id, char(0))=0)),
    capability_id TEXT CONSTRAINT channel_identity_registry_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    CONSTRAINT channel_identity_registry_label_check
        CHECK (length(trim(label)) BETWEEN 1 AND 100),
    CONSTRAINT channel_identity_registry_pkey PRIMARY KEY (id),
    CONSTRAINT channel_identity_registry_canonical_channel_id_fkey FOREIGN KEY (canonical_channel_id) REFERENCES upstream_channels (id) ON DELETE RESTRICT,
    CONSTRAINT channel_identity_registry_capability_id_fkey FOREIGN KEY (capability_id) REFERENCES channel_capabilities (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "codex_oauth_credentials" (
    channel_id TEXT NOT NULL CONSTRAINT codex_oauth_credentials_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id))) CHECK (channel_id IS NULL OR instr(channel_id, char(0))=0),
    label TEXT NOT NULL CONSTRAINT codex_oauth_credentials_label_storage CHECK (label IS NULL OR (length(label) <= 100)) CHECK (label IS NULL OR instr(label, char(0))=0),
    email TEXT CONSTRAINT codex_oauth_credentials_email_storage CHECK (email IS NULL OR (length(email) <= 320)) CHECK (email IS NULL OR instr(email, char(0))=0),
    account_id TEXT CONSTRAINT codex_oauth_credentials_account_id_storage CHECK (account_id IS NULL OR (length(account_id) <= 300)) CHECK (account_id IS NULL OR instr(account_id, char(0))=0),
    plan_type TEXT CONSTRAINT codex_oauth_credentials_plan_type_storage CHECK (plan_type IS NULL OR (length(plan_type) <= 100)) CHECK (plan_type IS NULL OR instr(plan_type, char(0))=0),
    is_fedramp INTEGER DEFAULT (0) NOT NULL CONSTRAINT codex_oauth_credentials_is_fedramp_storage CHECK (is_fedramp IS NULL OR (is_fedramp IN (0,1))),
    id_token TEXT NOT NULL CHECK (id_token IS NULL OR instr(id_token, char(0))=0),
    access_token TEXT NOT NULL CHECK (access_token IS NULL OR instr(access_token, char(0))=0),
    refresh_token TEXT NOT NULL CHECK (refresh_token IS NULL OR instr(refresh_token, char(0))=0),
    access_token_expires_at TEXT CONSTRAINT codex_oauth_credentials_access_token_expires_at_storage CHECK (access_token_expires_at IS NULL OR (ag_time_valid(access_token_expires_at))) CHECK (access_token_expires_at IS NULL OR instr(access_token_expires_at, char(0))=0),
    last_refreshed_at TEXT NOT NULL CONSTRAINT codex_oauth_credentials_last_refreshed_at_storage CHECK (last_refreshed_at IS NULL OR (ag_time_valid(last_refreshed_at))) CHECK (last_refreshed_at IS NULL OR instr(last_refreshed_at, char(0))=0),
    refresh_generation INTEGER DEFAULT (0) NOT NULL,
    reauth_required INTEGER DEFAULT (0) NOT NULL CONSTRAINT codex_oauth_credentials_reauth_required_storage CHECK (reauth_required IS NULL OR (reauth_required IN (0,1))),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT codex_oauth_credentials_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    quota_threshold_percent INTEGER DEFAULT (95) NOT NULL CONSTRAINT codex_oauth_credentials_quota_threshold_percent_storage CHECK (quota_threshold_percent IS NULL OR (quota_threshold_percent BETWEEN -32768 AND 32767)),
    runtime_status TEXT DEFAULT ('active') NOT NULL CHECK (runtime_status IS NULL OR instr(runtime_status, char(0))=0),
    quota_allowed INTEGER CONSTRAINT codex_oauth_credentials_quota_allowed_storage CHECK (quota_allowed IS NULL OR (quota_allowed IN (0,1))),
    quota_limit_reached INTEGER CONSTRAINT codex_oauth_credentials_quota_limit_reached_storage CHECK (quota_limit_reached IS NULL OR (quota_limit_reached IN (0,1))),
    primary_used_percent INTEGER CONSTRAINT codex_oauth_credentials_primary_used_percent_storage CHECK (primary_used_percent IS NULL OR (primary_used_percent BETWEEN -2147483648 AND 2147483647)),
    primary_window_seconds INTEGER CONSTRAINT codex_oauth_credentials_primary_window_seconds_storage CHECK (primary_window_seconds IS NULL OR (primary_window_seconds BETWEEN -2147483648 AND 2147483647)),
    primary_reset_at TEXT CONSTRAINT codex_oauth_credentials_primary_reset_at_storage CHECK (primary_reset_at IS NULL OR (ag_time_valid(primary_reset_at))) CHECK (primary_reset_at IS NULL OR instr(primary_reset_at, char(0))=0),
    secondary_used_percent INTEGER CONSTRAINT codex_oauth_credentials_secondary_used_percent_storage CHECK (secondary_used_percent IS NULL OR (secondary_used_percent BETWEEN -2147483648 AND 2147483647)),
    secondary_window_seconds INTEGER CONSTRAINT codex_oauth_credentials_secondary_window_seconds_storage CHECK (secondary_window_seconds IS NULL OR (secondary_window_seconds BETWEEN -2147483648 AND 2147483647)),
    secondary_reset_at TEXT CONSTRAINT codex_oauth_credentials_secondary_reset_at_storage CHECK (secondary_reset_at IS NULL OR (ag_time_valid(secondary_reset_at))) CHECK (secondary_reset_at IS NULL OR instr(secondary_reset_at, char(0))=0),
    quota_checked_at TEXT CONSTRAINT codex_oauth_credentials_quota_checked_at_storage CHECK (quota_checked_at IS NULL OR (ag_time_valid(quota_checked_at))) CHECK (quota_checked_at IS NULL OR instr(quota_checked_at, char(0))=0),
    last_error_code TEXT CONSTRAINT codex_oauth_credentials_last_error_code_storage CHECK (last_error_code IS NULL OR (length(last_error_code) <= 100)) CHECK (last_error_code IS NULL OR instr(last_error_code, char(0))=0),
    last_error_summary TEXT CONSTRAINT codex_oauth_credentials_last_error_summary_storage CHECK (last_error_summary IS NULL OR (length(last_error_summary) <= 1000)) CHECK (last_error_summary IS NULL OR instr(last_error_summary, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_oauth_credentials_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_oauth_credentials_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    user_id TEXT CONSTRAINT codex_oauth_credentials_user_id_storage CHECK (user_id IS NULL OR (length(user_id) <= 300)) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    deleted_at TEXT CONSTRAINT codex_oauth_credentials_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    quota_reset_credits_available INTEGER, proxy_id TEXT
    REFERENCES proxies(id) ON DELETE RESTRICT CHECK(proxy_id IS NULL OR ag_uuid_valid(proxy_id)), available_models TEXT NOT NULL DEFAULT '[]'
    CHECK(json_valid(available_models) AND json_type(available_models)='array'),
    CONSTRAINT codex_oauth_credentials_access_token_check CHECK ((length(access_token) > 0)),
    CONSTRAINT codex_oauth_credentials_account_id_check CHECK ((trim(account_id) <> '')),
    CONSTRAINT codex_oauth_credentials_account_or_user_check CHECK (((account_id IS NOT NULL) OR (user_id IS NOT NULL))),
    CONSTRAINT codex_oauth_credentials_check CHECK (((NOT reauth_required) OR ag_array_contains(json_array('unavailable', 'disabled'), runtime_status))),
    CONSTRAINT codex_oauth_credentials_id_token_check CHECK ((length(id_token) > 0)),
    CONSTRAINT codex_oauth_credentials_label_check CHECK ((trim(label) <> '')),
    CONSTRAINT codex_oauth_credentials_primary_used_percent_check CHECK (((primary_used_percent >= 0) AND (primary_used_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_primary_window_seconds_check CHECK ((primary_window_seconds > 0)),
    CONSTRAINT codex_oauth_credentials_quota_threshold_percent_check CHECK (((quota_threshold_percent >= 1) AND (quota_threshold_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_refresh_generation_check CHECK ((refresh_generation >= 0)),
    CONSTRAINT codex_oauth_credentials_refresh_token_check CHECK ((length(refresh_token) > 0)),
    CONSTRAINT codex_oauth_credentials_reset_credits_check CHECK (((quota_reset_credits_available IS NULL) OR (quota_reset_credits_available >= 0))),
    CONSTRAINT codex_oauth_credentials_runtime_status_check CHECK (ag_array_contains(json_array('active', 'draining', 'unavailable', 'disabled'), runtime_status)),
    CONSTRAINT codex_oauth_credentials_secondary_used_percent_check CHECK (((secondary_used_percent >= 0) AND (secondary_used_percent <= 100))),
    CONSTRAINT codex_oauth_credentials_secondary_window_seconds_check CHECK ((secondary_window_seconds > 0)),
    CONSTRAINT codex_oauth_credentials_user_id_check CHECK (((user_id IS NULL) OR (trim(user_id) <> ''))),
    CONSTRAINT codex_oauth_credentials_pkey PRIMARY KEY (channel_id),
    CONSTRAINT codex_oauth_credentials_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES upstream_credentials (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "codex_oauth_flows" (
    id TEXT NOT NULL CONSTRAINT codex_oauth_flows_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    actor_user_id TEXT NOT NULL CONSTRAINT codex_oauth_flows_actor_user_id_storage CHECK (actor_user_id IS NULL OR (ag_uuid_valid(actor_user_id))) CHECK (actor_user_id IS NULL OR instr(actor_user_id, char(0))=0),
    label TEXT NOT NULL CONSTRAINT codex_oauth_flows_label_storage CHECK (label IS NULL OR (length(label) <= 100)) CHECK (label IS NULL OR instr(label, char(0))=0),
    proxy_id TEXT CONSTRAINT codex_oauth_flows_proxy_id_storage CHECK (proxy_id IS NULL OR (ag_uuid_valid(proxy_id))) CHECK (proxy_id IS NULL OR instr(proxy_id, char(0))=0),
    quota_threshold_percent INTEGER NOT NULL CONSTRAINT codex_oauth_flows_quota_threshold_percent_storage CHECK (quota_threshold_percent IS NULL OR (quota_threshold_percent BETWEEN -32768 AND 32767)),
    state_hash BLOB NOT NULL,
    code_verifier TEXT NOT NULL CHECK (code_verifier IS NULL OR instr(code_verifier, char(0))=0),
    expires_at TEXT NOT NULL CONSTRAINT codex_oauth_flows_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    completed_at TEXT CONSTRAINT codex_oauth_flows_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_oauth_flows_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    CONSTRAINT codex_oauth_flows_code_verifier_check CHECK (((length(code_verifier) >= 43) AND (length(code_verifier) <= 128))),
    CONSTRAINT codex_oauth_flows_label_check CHECK ((trim(label) <> '')),
    CONSTRAINT codex_oauth_flows_quota_threshold_percent_check CHECK (((quota_threshold_percent >= 1) AND (quota_threshold_percent <= 100))),
    CONSTRAINT codex_oauth_flows_state_hash_check CHECK ((length(state_hash) = 32)),
    CONSTRAINT codex_oauth_flows_pkey PRIMARY KEY (id),
    CONSTRAINT codex_oauth_flows_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT codex_oauth_flows_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES proxies (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE codex_quota_reset_events (
    id TEXT NOT NULL CONSTRAINT codex_quota_reset_events_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    credential_id TEXT NOT NULL CONSTRAINT codex_quota_reset_events_credential_id_storage CHECK (credential_id IS NULL OR (ag_uuid_valid(credential_id))) CHECK (credential_id IS NULL OR instr(credential_id, char(0))=0),
    actor_user_id TEXT NOT NULL CONSTRAINT codex_quota_reset_events_actor_user_id_storage CHECK (actor_user_id IS NULL OR (ag_uuid_valid(actor_user_id))) CHECK (actor_user_id IS NULL OR instr(actor_user_id, char(0))=0),
    requested_at TEXT NOT NULL CONSTRAINT codex_quota_reset_events_requested_at_storage CHECK (requested_at IS NULL OR (ag_time_valid(requested_at))) CHECK (requested_at IS NULL OR instr(requested_at, char(0))=0),
    outcome TEXT NOT NULL CHECK (outcome IS NULL OR instr(outcome, char(0))=0),
    windows_reset INTEGER NOT NULL CONSTRAINT codex_quota_reset_events_windows_reset_storage CHECK (windows_reset IS NULL OR (windows_reset BETWEEN -2147483648 AND 2147483647)),
    primary_applied_at TEXT CONSTRAINT codex_quota_reset_events_primary_applied_at_storage CHECK (primary_applied_at IS NULL OR (ag_time_valid(primary_applied_at))) CHECK (primary_applied_at IS NULL OR instr(primary_applied_at, char(0))=0),
    secondary_applied_at TEXT CONSTRAINT codex_quota_reset_events_secondary_applied_at_storage CHECK (secondary_applied_at IS NULL OR (ag_time_valid(secondary_applied_at))) CHECK (secondary_applied_at IS NULL OR instr(secondary_applied_at, char(0))=0),
    correlation_id TEXT NOT NULL CONSTRAINT codex_quota_reset_events_correlation_id_storage CHECK (correlation_id IS NULL OR (ag_uuid_valid(correlation_id))) CHECK (correlation_id IS NULL OR instr(correlation_id, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_quota_reset_events_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    CONSTRAINT codex_quota_reset_events_outcome_check CHECK (ag_array_contains(json_array('reset', 'nothing_to_reset', 'no_credit', 'already_redeemed'), outcome)),
    CONSTRAINT codex_quota_reset_events_windows_reset_check CHECK (((windows_reset >= 0) AND (windows_reset <= 2))),
    CONSTRAINT codex_quota_reset_events_pkey PRIMARY KEY (id),
    CONSTRAINT codex_quota_reset_events_actor_user_id_fkey FOREIGN KEY (actor_user_id) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT codex_quota_reset_events_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE codex_quota_window_periods (
    id TEXT NOT NULL CONSTRAINT codex_quota_window_periods_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    credential_id TEXT NOT NULL CONSTRAINT codex_quota_window_periods_credential_id_storage CHECK (credential_id IS NULL OR (ag_uuid_valid(credential_id))) CHECK (credential_id IS NULL OR instr(credential_id, char(0))=0),
    window_kind TEXT NOT NULL CHECK (window_kind IS NULL OR instr(window_kind, char(0))=0),
    window_seconds INTEGER NOT NULL CONSTRAINT codex_quota_window_periods_window_seconds_storage CHECK (window_seconds IS NULL OR (window_seconds BETWEEN -2147483648 AND 2147483647)),
    started_at TEXT NOT NULL CONSTRAINT codex_quota_window_periods_started_at_storage CHECK (started_at IS NULL OR (ag_time_valid(started_at))) CHECK (started_at IS NULL OR instr(started_at, char(0))=0),
    scheduled_reset_at TEXT NOT NULL CONSTRAINT codex_quota_window_periods_scheduled_reset_at_storage CHECK (scheduled_reset_at IS NULL OR (ag_time_valid(scheduled_reset_at))) CHECK (scheduled_reset_at IS NULL OR instr(scheduled_reset_at, char(0))=0),
    ended_at TEXT CONSTRAINT codex_quota_window_periods_ended_at_storage CHECK (ended_at IS NULL OR (ag_time_valid(ended_at))) CHECK (ended_at IS NULL OR instr(ended_at, char(0))=0),
    reset_reason TEXT CHECK (reset_reason IS NULL OR instr(reset_reason, char(0))=0),
    initial_used_percent INTEGER NOT NULL CONSTRAINT codex_quota_window_periods_initial_used_percent_storage CHECK (initial_used_percent IS NULL OR (initial_used_percent BETWEEN -2147483648 AND 2147483647)),
    last_used_percent INTEGER NOT NULL CONSTRAINT codex_quota_window_periods_last_used_percent_storage CHECK (last_used_percent IS NULL OR (last_used_percent BETWEEN -2147483648 AND 2147483647)),
    first_observed_at TEXT NOT NULL CONSTRAINT codex_quota_window_periods_first_observed_at_storage CHECK (first_observed_at IS NULL OR (ag_time_valid(first_observed_at))) CHECK (first_observed_at IS NULL OR instr(first_observed_at, char(0))=0),
    last_observed_at TEXT NOT NULL CONSTRAINT codex_quota_window_periods_last_observed_at_storage CHECK (last_observed_at IS NULL OR (ag_time_valid(last_observed_at))) CHECK (last_observed_at IS NULL OR instr(last_observed_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_quota_window_periods_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT codex_quota_window_periods_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT codex_quota_window_periods_check CHECK ((scheduled_reset_at > started_at)),
    CONSTRAINT codex_quota_window_periods_check1 CHECK (((ended_at IS NULL) OR (ended_at >= started_at))),
    CONSTRAINT codex_quota_window_periods_check2 CHECK ((((ended_at IS NULL) AND (reset_reason IS NULL)) OR ((ended_at IS NOT NULL) AND (reset_reason IS NOT NULL)))),
    CONSTRAINT codex_quota_window_periods_check3 CHECK ((last_observed_at >= first_observed_at)),
    CONSTRAINT codex_quota_window_periods_initial_used_percent_check CHECK (((initial_used_percent >= 0) AND (initial_used_percent <= 100))),
    CONSTRAINT codex_quota_window_periods_last_used_percent_check CHECK (((last_used_percent >= 0) AND (last_used_percent <= 100))),
    CONSTRAINT codex_quota_window_periods_reset_reason_check CHECK (ag_array_contains(json_array('natural', 'manual', 'openai_official'), reset_reason)),
    CONSTRAINT codex_quota_window_periods_window_kind_check CHECK (ag_array_contains(json_array('primary', 'secondary'), window_kind)),
    CONSTRAINT codex_quota_window_periods_window_seconds_check CHECK ((window_seconds > 0)),
    CONSTRAINT codex_quota_window_periods_pkey PRIMARY KEY (id),
    CONSTRAINT codex_quota_window_periods_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES codex_oauth_credentials (channel_id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE config_templates (
    id TEXT NOT NULL CONSTRAINT config_templates_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT config_templates_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    description TEXT CHECK (description IS NULL OR instr(description, char(0))=0),
    document TEXT NOT NULL CONSTRAINT config_templates_document_storage CHECK (document IS NULL OR (ag_json_valid(document))) CHECK (document IS NULL OR instr(document, char(0))=0),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT config_templates_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT config_templates_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT config_templates_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT config_templates_document_check CHECK ((json_type(document) = 'object')),
    CONSTRAINT config_templates_name_key UNIQUE (name),
    CONSTRAINT config_templates_pkey PRIMARY KEY (id)
) STRICT;

CREATE TABLE group_identity_registry (
    id TEXT NOT NULL CONSTRAINT group_identity_registry_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    label TEXT NOT NULL CONSTRAINT group_identity_registry_label_storage CHECK (label IS NULL OR (length(label) <= 100 AND instr(label, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT group_identity_registry_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    canonical_group_id TEXT CONSTRAINT group_identity_registry_canonical_group_id_storage CHECK (canonical_group_id IS NULL OR (ag_uuid_valid(canonical_group_id) AND instr(canonical_group_id, char(0))=0)),
    CONSTRAINT group_identity_registry_label_check
        CHECK (length(trim(label)) BETWEEN 1 AND 100),
    CONSTRAINT group_identity_registry_pkey PRIMARY KEY (id),
    CONSTRAINT group_identity_registry_canonical_group_id_fkey FOREIGN KEY (canonical_group_id) REFERENCES routing_groups (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "model_capability_candidates" (
    tier_id TEXT NOT NULL CONSTRAINT model_capability_candidates_tier_id_storage CHECK (tier_id IS NULL OR (ag_uuid_valid(tier_id) AND instr(tier_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_capability_candidates_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    capability_id TEXT NOT NULL CONSTRAINT model_capability_candidates_capability_id_storage CHECK (capability_id IS NULL OR (ag_uuid_valid(capability_id) AND instr(capability_id, char(0))=0)),
    upstream_model TEXT NOT NULL CONSTRAINT model_capability_candidates_upstream_model_storage CHECK (upstream_model IS NULL OR (length(upstream_model) <= 300 AND instr(upstream_model, char(0))=0)),
    weight INTEGER NOT NULL CONSTRAINT model_capability_candidates_weight_storage CHECK (weight IS NULL OR weight BETWEEN -2147483648 AND 2147483647),
    CONSTRAINT model_capability_candidates_pkey PRIMARY KEY (tier_id, capability_id, upstream_model),
    CONSTRAINT model_capability_candidates_tier_fk
        FOREIGN KEY (tier_id, operation)
        REFERENCES model_capability_tiers (id, operation) ON DELETE CASCADE,
    CONSTRAINT model_capability_candidates_capability_fk
        FOREIGN KEY (capability_id, operation)
        REFERENCES channel_capabilities (id, operation) ON DELETE RESTRICT,
    CONSTRAINT model_capability_candidates_upstream_model_check
        CHECK (trim(upstream_model) <> ''),
    CONSTRAINT model_capability_candidates_weight_check
        CHECK (weight > 0)
) STRICT;

CREATE TABLE "model_capability_tiers" (
    id TEXT NOT NULL CONSTRAINT model_capability_tiers_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    rule_id TEXT NOT NULL CONSTRAINT model_capability_tiers_rule_id_storage CHECK (rule_id IS NULL OR (ag_uuid_valid(rule_id) AND instr(rule_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_capability_tiers_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    priority INTEGER NOT NULL CONSTRAINT model_capability_tiers_priority_storage CHECK (priority IS NULL OR priority BETWEEN -2147483648 AND 2147483647),
    strategy TEXT NOT NULL CONSTRAINT model_capability_tiers_strategy_storage CHECK (strategy IS NULL OR instr(strategy, char(0))=0),
    CONSTRAINT model_capability_tiers_rule_priority_key
        UNIQUE (rule_id, priority),
    CONSTRAINT model_capability_tiers_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_capability_tiers_priority_check
        CHECK (priority >= 0),
    CONSTRAINT model_capability_tiers_strategy_check
        CHECK (ag_array_contains(json_array('weighted_random', 'weighted_round_robin'), strategy)),
    CONSTRAINT model_capability_tiers_pkey PRIMARY KEY (id),
    CONSTRAINT model_capability_tiers_rule_fk
        FOREIGN KEY (rule_id, operation)
        REFERENCES model_operation_rules (id, operation) ON DELETE CASCADE
) STRICT;

CREATE TABLE "model_operation_rules" (
    id TEXT NOT NULL CONSTRAINT model_operation_rules_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    model_routing_profile_id TEXT NOT NULL CONSTRAINT model_operation_rules_model_routing_profile_id_storage CHECK (model_routing_profile_id IS NULL OR (ag_uuid_valid(model_routing_profile_id) AND instr(model_routing_profile_id, char(0))=0)),
    operation TEXT NOT NULL CONSTRAINT model_operation_rules_operation_storage CHECK (operation IS NULL OR instr(operation, char(0))=0),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT model_operation_rules_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_operation_rules_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_operation_rules_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    CONSTRAINT model_operation_rules_profile_operation_key
        UNIQUE (model_routing_profile_id, operation),
    CONSTRAINT model_operation_rules_id_operation_key
        UNIQUE (id, operation),
    CONSTRAINT model_operation_rules_operation_check CHECK (operation IN ('chat_completion','responses','responses-ws','web_search','images_edit','images_generation')),
    CONSTRAINT model_operation_rules_pkey PRIMARY KEY (id),
    CONSTRAINT model_operation_rules_model_routing_profile_id_fkey FOREIGN KEY (model_routing_profile_id) REFERENCES model_routing_profiles (id) ON DELETE CASCADE
) STRICT;

CREATE TABLE model_routing_profiles (
    id TEXT NOT NULL CONSTRAINT model_routing_profiles_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    model_id TEXT NOT NULL CONSTRAINT model_routing_profiles_model_id_storage CHECK (model_id IS NULL OR (ag_uuid_valid(model_id))) CHECK (model_id IS NULL OR instr(model_id, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_routing_profiles_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT model_routing_profiles_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT model_routing_profiles_model_id_key UNIQUE (model_id),
    CONSTRAINT model_routing_profiles_pkey PRIMARY KEY (id),
    CONSTRAINT model_routing_profiles_model_id_fkey FOREIGN KEY (model_id) REFERENCES models (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE model_rule_identity_registry (
    id TEXT NOT NULL PRIMARY KEY CHECK (ag_uuid_valid(id) AND instr(id, char(0))=0),
    label TEXT NOT NULL CHECK (length(trim(label)) > 0 AND instr(label, char(0))=0),
    created_at TEXT NOT NULL CHECK (ag_time_valid(created_at) AND instr(created_at, char(0))=0),
    canonical_rule_id TEXT REFERENCES model_operation_rules (id) ON DELETE RESTRICT
        CHECK (canonical_rule_id IS NULL OR (ag_uuid_valid(canonical_rule_id) AND instr(canonical_rule_id, char(0))=0))
) STRICT;

CREATE TABLE models (
    id TEXT NOT NULL CONSTRAINT models_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    source_model_id TEXT NOT NULL CONSTRAINT models_source_model_id_storage CHECK (source_model_id IS NULL OR (length(source_model_id) <= 300)) CHECK (source_model_id IS NULL OR instr(source_model_id, char(0))=0),
    display_name TEXT NOT NULL CONSTRAINT models_display_name_storage CHECK (display_name IS NULL OR (length(display_name) <= 300)) CHECK (display_name IS NULL OR instr(display_name, char(0))=0),
    provider_name TEXT CONSTRAINT models_provider_name_storage CHECK (provider_name IS NULL OR (length(provider_name) <= 200)) CHECK (provider_name IS NULL OR instr(provider_name, char(0))=0),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT models_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    currency TEXT DEFAULT ('USD') NOT NULL CONSTRAINT models_currency_storage CHECK (currency IS NULL OR (length(currency) <= 3)) CHECK (currency IS NULL OR instr(currency, char(0))=0),
    price_unit_tokens INTEGER NOT NULL,
    input_unit_price TEXT NOT NULL CONSTRAINT models_input_unit_price_storage CHECK (input_unit_price IS NULL OR (ag_decimal_valid(input_unit_price, 24, 12))) CHECK (input_unit_price IS NULL OR instr(input_unit_price, char(0))=0),
    cached_input_unit_price TEXT NOT NULL CONSTRAINT models_cached_input_unit_price_storage CHECK (cached_input_unit_price IS NULL OR (ag_decimal_valid(cached_input_unit_price, 24, 12))) CHECK (cached_input_unit_price IS NULL OR instr(cached_input_unit_price, char(0))=0),
    cache_write_unit_price TEXT NOT NULL CONSTRAINT models_cache_write_unit_price_storage CHECK (cache_write_unit_price IS NULL OR (ag_decimal_valid(cache_write_unit_price, 24, 12))) CHECK (cache_write_unit_price IS NULL OR instr(cache_write_unit_price, char(0))=0),
    output_unit_price TEXT NOT NULL CONSTRAINT models_output_unit_price_storage CHECK (output_unit_price IS NULL OR (ag_decimal_valid(output_unit_price, 24, 12))) CHECK (output_unit_price IS NULL OR instr(output_unit_price, char(0))=0),
    price_effective_at TEXT NOT NULL CONSTRAINT models_price_effective_at_storage CHECK (price_effective_at IS NULL OR (ag_time_valid(price_effective_at))) CHECK (price_effective_at IS NULL OR instr(price_effective_at, char(0))=0),
    source_payload TEXT DEFAULT ('{}') NOT NULL CONSTRAINT models_source_payload_storage CHECK (source_payload IS NULL OR (ag_json_valid(source_payload))) CHECK (source_payload IS NULL OR instr(source_payload, char(0))=0),
    last_synced_at TEXT CONSTRAINT models_last_synced_at_storage CHECK (last_synced_at IS NULL OR (ag_time_valid(last_synced_at))) CHECK (last_synced_at IS NULL OR instr(last_synced_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT models_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT models_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    advanced_billing TEXT DEFAULT ('{"long_context_tiers": [], "request_multipliers": []}') NOT NULL CONSTRAINT models_advanced_billing_storage CHECK (advanced_billing IS NULL OR (ag_json_valid(advanced_billing))) CHECK (advanced_billing IS NULL OR instr(advanced_billing, char(0))=0),
    deleted_at TEXT CONSTRAINT models_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    deleted_by TEXT CONSTRAINT models_deleted_by_storage CHECK (deleted_by IS NULL OR (ag_uuid_valid(deleted_by))) CHECK (deleted_by IS NULL OR instr(deleted_by, char(0))=0),
    CONSTRAINT models_advanced_billing_object CHECK ((json_type(advanced_billing) = 'object')),
    CONSTRAINT models_cache_write_unit_price_check CHECK ((ag_decimal_cmp(cache_write_unit_price, '0') >= 0)),
    CONSTRAINT models_cached_input_unit_price_check CHECK ((ag_decimal_cmp(cached_input_unit_price, '0') >= 0)),
    CONSTRAINT models_currency_usd_only CHECK ((currency = 'USD')),
    CONSTRAINT models_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT models_deleted_state_check CHECK (((deleted_at IS NULL) OR (NOT enabled))),
    CONSTRAINT models_input_unit_price_check CHECK ((ag_decimal_cmp(input_unit_price, '0') >= 0)),
    CONSTRAINT models_output_unit_price_check CHECK ((ag_decimal_cmp(output_unit_price, '0') >= 0)),
    CONSTRAINT models_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT models_source_payload_check CHECK ((json_type(source_payload) = 'object')),
    CONSTRAINT models_pkey PRIMARY KEY (id),
    CONSTRAINT models_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE plugin_artifacts (
    plugin_id TEXT NOT NULL CHECK (plugin_id <> 'general'),
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    version TEXT NOT NULL,
    manifest TEXT NOT NULL CHECK (json_valid(manifest) AND json_type(manifest) = 'object'),
    created_at TEXT NOT NULL DEFAULT (ag_now()),
    deleted_at TEXT,
    PRIMARY KEY (plugin_id, digest)
) STRICT;

CREATE TABLE plugin_install_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    actor_user_id TEXT NOT NULL REFERENCES users(id),
    operation TEXT NOT NULL CHECK (operation IN ('install','discover')),
    status TEXT NOT NULL CHECK (status IN ('queued','running','succeeded','failed')),
    plugin_id TEXT,
    artifact_digest TEXT,
    error_code TEXT,
    created_at TEXT NOT NULL DEFAULT (ag_now()),
    updated_at TEXT NOT NULL DEFAULT (ag_now()),
    CHECK (operation <> 'install' OR status <> 'succeeded' OR
           (plugin_id IS NOT NULL AND artifact_digest IS NOT NULL))
) STRICT;

CREATE TABLE plugin_settings (
    plugin_id TEXT PRIMARY KEY NOT NULL CHECK (plugin_id <> 'general'),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    settings_value TEXT NOT NULL CHECK (json_valid(settings_value) AND json_type(settings_value) = 'object'),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TEXT NOT NULL DEFAULT (ag_now())
) STRICT;

CREATE TABLE plugin_states (
    plugin_id TEXT PRIMARY KEY NOT NULL CHECK (plugin_id <> 'general'),
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
    artifact_digest TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision > 0),
    updated_at TEXT NOT NULL DEFAULT (ag_now()),
    CHECK (NOT enabled OR artifact_digest IS NOT NULL),
    FOREIGN KEY (plugin_id, artifact_digest) REFERENCES plugin_artifacts(plugin_id, digest)
) STRICT;

CREATE TABLE proxies (
    id TEXT NOT NULL CONSTRAINT proxies_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT proxies_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    proxy_url TEXT NOT NULL CHECK (proxy_url IS NULL OR instr(proxy_url, char(0))=0),
    username TEXT CHECK (username IS NULL OR instr(username, char(0))=0),
    password TEXT CHECK (password IS NULL OR instr(password, char(0))=0),
    no_proxy_hosts TEXT DEFAULT ('[]') NOT NULL CONSTRAINT proxies_no_proxy_hosts_storage CHECK (no_proxy_hosts IS NULL OR (ag_array_valid(no_proxy_hosts, 'text'))) CHECK (no_proxy_hosts IS NULL OR instr(no_proxy_hosts, char(0))=0),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT proxies_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT proxies_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT proxies_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT proxies_proxy_url_check CHECK (ag_regex('^(https?|socks4a?|socks5h?)://', proxy_url)),
    CONSTRAINT proxies_name_key UNIQUE (name),
    CONSTRAINT proxies_pkey PRIMARY KEY (id)
) STRICT;

CREATE TABLE registration_invitation_codes (
    id TEXT NOT NULL CONSTRAINT registration_invitation_codes_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT registration_invitation_codes_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    code_hash BLOB NOT NULL,
    max_uses INTEGER,
    used_count INTEGER DEFAULT (0) NOT NULL,
    expires_at TEXT CONSTRAINT registration_invitation_codes_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    enabled INTEGER DEFAULT (1) NOT NULL CONSTRAINT registration_invitation_codes_enabled_storage CHECK (enabled IS NULL OR (enabled IN (0,1))),
    user_group_id TEXT NOT NULL CONSTRAINT registration_invitation_codes_user_group_id_storage CHECK (user_group_id IS NULL OR (ag_uuid_valid(user_group_id))) CHECK (user_group_id IS NULL OR instr(user_group_id, char(0))=0),
    initial_balance_amount TEXT DEFAULT ('0') NOT NULL CONSTRAINT registration_invitation_codes_initial_balance_amount_storage CHECK (initial_balance_amount IS NULL OR (ag_decimal_valid(initial_balance_amount, 24, 8))) CHECK (initial_balance_amount IS NULL OR instr(initial_balance_amount, char(0))=0),
    created_by TEXT NOT NULL CONSTRAINT registration_invitation_codes_created_by_storage CHECK (created_by IS NULL OR (ag_uuid_valid(created_by))) CHECK (created_by IS NULL OR instr(created_by, char(0))=0),
    last_used_at TEXT CONSTRAINT registration_invitation_codes_last_used_at_storage CHECK (last_used_at IS NULL OR (ag_time_valid(last_used_at))) CHECK (last_used_at IS NULL OR instr(last_used_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT registration_invitation_codes_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT registration_invitation_codes_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT registration_invitation_codes_check CHECK (((max_uses IS NULL) OR (used_count <= max_uses))),
    CONSTRAINT registration_invitation_codes_initial_balance_amount_check CHECK ((ag_decimal_cmp(initial_balance_amount, '0') >= 0)),
    CONSTRAINT registration_invitation_codes_max_uses_check CHECK (((max_uses IS NULL) OR (max_uses > 0))),
    CONSTRAINT registration_invitation_codes_used_count_check CHECK ((used_count >= 0)),
    CONSTRAINT registration_invitation_codes_code_hash_key UNIQUE (code_hash),
    CONSTRAINT registration_invitation_codes_name_key UNIQUE (name),
    CONSTRAINT registration_invitation_codes_pkey PRIMARY KEY (id),
    CONSTRAINT registration_invitation_codes_created_by_fkey FOREIGN KEY (created_by) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT registration_invitation_codes_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES user_groups (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE request_log_ingest (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    request_log_id TEXT NOT NULL CONSTRAINT request_log_ingest_request_log_id_storage CHECK (request_log_id IS NULL OR (ag_uuid_valid(request_log_id))) CHECK (request_log_id IS NULL OR instr(request_log_id, char(0))=0),
    schema_version INTEGER NOT NULL CONSTRAINT request_log_ingest_schema_version_storage CHECK (schema_version IS NULL OR (schema_version BETWEEN -32768 AND 32767)),
    payload BLOB NOT NULL,
    staged_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT request_log_ingest_staged_at_storage CHECK (staged_at IS NULL OR (ag_time_valid(staged_at))) CHECK (staged_at IS NULL OR instr(staged_at, char(0))=0),
    attempt_count INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_log_ingest_attempt_count_storage CHECK (attempt_count IS NULL OR (attempt_count BETWEEN -2147483648 AND 2147483647)),
    next_attempt_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT request_log_ingest_next_attempt_at_storage CHECK (next_attempt_at IS NULL OR (ag_time_valid(next_attempt_at))) CHECK (next_attempt_at IS NULL OR instr(next_attempt_at, char(0))=0),
    last_error_code TEXT CONSTRAINT request_log_ingest_last_error_code_storage CHECK (last_error_code IS NULL OR (length(last_error_code) <= 100)) CHECK (last_error_code IS NULL OR instr(last_error_code, char(0))=0),
    metered_at TEXT CONSTRAINT request_log_ingest_metered_at_storage CHECK (metered_at IS NULL OR (ag_time_valid(metered_at))) CHECK (metered_at IS NULL OR instr(metered_at, char(0))=0),
    metering_attempt_count INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_log_ingest_metering_attempt_count_storage CHECK (metering_attempt_count IS NULL OR (metering_attempt_count BETWEEN -2147483648 AND 2147483647)),
    metering_next_attempt_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT request_log_ingest_metering_next_attempt_at_storage CHECK (metering_next_attempt_at IS NULL OR (ag_time_valid(metering_next_attempt_at))) CHECK (metering_next_attempt_at IS NULL OR instr(metering_next_attempt_at, char(0))=0),
    metering_last_error_code TEXT CHECK (metering_last_error_code IS NULL OR instr(metering_last_error_code, char(0))=0),
    CONSTRAINT request_log_ingest_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT request_log_ingest_schema_version_check CHECK ((schema_version > 0))
) STRICT;

CREATE TABLE "request_logs" (
    id TEXT NOT NULL CONSTRAINT request_logs_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    started_at TEXT NOT NULL CONSTRAINT request_logs_started_at_storage CHECK (started_at IS NULL OR (ag_time_valid(started_at))) CHECK (started_at IS NULL OR instr(started_at, char(0))=0),
    completed_at TEXT NOT NULL CONSTRAINT request_logs_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT request_logs_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    api_key_id TEXT NOT NULL CONSTRAINT request_logs_api_key_id_storage CHECK (api_key_id IS NULL OR (ag_uuid_valid(api_key_id))) CHECK (api_key_id IS NULL OR instr(api_key_id, char(0))=0),
    api_format TEXT COLLATE ag_api_format NOT NULL CONSTRAINT request_logs_api_format_storage CHECK (api_format IS NULL OR (api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images'))) CHECK (api_format IS NULL OR instr(api_format, char(0))=0),
    client_model TEXT NOT NULL CONSTRAINT request_logs_client_model_storage CHECK (client_model IS NULL OR (length(client_model) <= 300)) CHECK (client_model IS NULL OR instr(client_model, char(0))=0),
    upstream_model TEXT CONSTRAINT request_logs_upstream_model_storage CHECK (upstream_model IS NULL OR (length(upstream_model) <= 300)) CHECK (upstream_model IS NULL OR instr(upstream_model, char(0))=0),
    model_rule_id TEXT CONSTRAINT request_logs_model_rule_id_storage CHECK (model_rule_id IS NULL OR (ag_uuid_valid(model_rule_id))) CHECK (model_rule_id IS NULL OR instr(model_rule_id, char(0))=0),
    channel_group_id TEXT CONSTRAINT request_logs_channel_group_id_storage CHECK (channel_group_id IS NULL OR (ag_uuid_valid(channel_group_id))) CHECK (channel_group_id IS NULL OR instr(channel_group_id, char(0))=0),
    channel_id TEXT CONSTRAINT request_logs_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id))) CHECK (channel_id IS NULL OR instr(channel_id, char(0))=0),
    outcome TEXT NOT NULL CHECK (outcome IS NULL OR instr(outcome, char(0))=0),
    response_status_code INTEGER CONSTRAINT request_logs_response_status_code_storage CHECK (response_status_code IS NULL OR (response_status_code BETWEEN -32768 AND 32767)),
    ttft_ms INTEGER CONSTRAINT request_logs_ttft_ms_storage CHECK (ttft_ms IS NULL OR (ttft_ms BETWEEN -2147483648 AND 2147483647)),
    total_duration_ms INTEGER CONSTRAINT request_logs_total_duration_ms_storage CHECK (total_duration_ms IS NULL OR (total_duration_ms BETWEEN -2147483648 AND 2147483647)),
    output_tokens_per_second TEXT CONSTRAINT request_logs_output_tokens_per_second_storage CHECK (output_tokens_per_second IS NULL OR (ag_decimal_valid(output_tokens_per_second, 14, 4))) CHECK (output_tokens_per_second IS NULL OR instr(output_tokens_per_second, char(0))=0),
    input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_tokens INTEGER,
    output_tokens INTEGER,
    model_id TEXT CONSTRAINT request_logs_model_id_storage CHECK (model_id IS NULL OR (ag_uuid_valid(model_id))) CHECK (model_id IS NULL OR instr(model_id, char(0))=0),
    currency TEXT CONSTRAINT request_logs_currency_storage CHECK (currency IS NULL OR (length(currency) <= 3)) CHECK (currency IS NULL OR instr(currency, char(0))=0),
    price_unit_tokens INTEGER,
    price_effective_at TEXT CONSTRAINT request_logs_price_effective_at_storage CHECK (price_effective_at IS NULL OR (ag_time_valid(price_effective_at))) CHECK (price_effective_at IS NULL OR instr(price_effective_at, char(0))=0),
    input_unit_price TEXT CONSTRAINT request_logs_input_unit_price_storage CHECK (input_unit_price IS NULL OR (ag_decimal_valid(input_unit_price, 24, 12))) CHECK (input_unit_price IS NULL OR instr(input_unit_price, char(0))=0),
    cached_input_unit_price TEXT CONSTRAINT request_logs_cached_input_unit_price_storage CHECK (cached_input_unit_price IS NULL OR (ag_decimal_valid(cached_input_unit_price, 24, 12))) CHECK (cached_input_unit_price IS NULL OR instr(cached_input_unit_price, char(0))=0),
    cache_write_unit_price TEXT CONSTRAINT request_logs_cache_write_unit_price_storage CHECK (cache_write_unit_price IS NULL OR (ag_decimal_valid(cache_write_unit_price, 24, 12))) CHECK (cache_write_unit_price IS NULL OR instr(cache_write_unit_price, char(0))=0),
    output_unit_price TEXT CONSTRAINT request_logs_output_unit_price_storage CHECK (output_unit_price IS NULL OR (ag_decimal_valid(output_unit_price, 24, 12))) CHECK (output_unit_price IS NULL OR instr(output_unit_price, char(0))=0),
    cost_amount TEXT CONSTRAINT request_logs_cost_amount_storage CHECK (cost_amount IS NULL OR (ag_decimal_valid(cost_amount, 24, 8))) CHECK (cost_amount IS NULL OR instr(cost_amount, char(0))=0),
    error_code TEXT CONSTRAINT request_logs_error_code_storage CHECK (error_code IS NULL OR (length(error_code) <= 100)) CHECK (error_code IS NULL OR instr(error_code, char(0))=0),
    error_summary TEXT CONSTRAINT request_logs_error_summary_storage CHECK (error_summary IS NULL OR (length(error_summary) <= 16384)) CHECK (error_summary IS NULL OR instr(error_summary, char(0))=0),
    request_source TEXT DEFAULT ('client') NOT NULL CHECK (request_source IS NULL OR instr(request_source, char(0))=0),
    request_protocol TEXT DEFAULT ('non_stream') NOT NULL CHECK (request_protocol IS NULL OR instr(request_protocol, char(0))=0),
    reasoning_tokens INTEGER,
    reasoning_effort TEXT CONSTRAINT request_logs_reasoning_effort_storage CHECK (reasoning_effort IS NULL OR (length(reasoning_effort) <= 32)) CHECK (reasoning_effort IS NULL OR instr(reasoning_effort, char(0))=0),
    fast_mode INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_logs_fast_mode_storage CHECK (fast_mode IS NULL OR (fast_mode IN (0,1))),
    api_operation TEXT NOT NULL CHECK (api_operation IS NULL OR instr(api_operation, char(0))=0),
    peak_pricing INTEGER DEFAULT (0) NOT NULL CONSTRAINT request_logs_peak_pricing_storage CHECK (peak_pricing IS NULL OR (peak_pricing IN (0,1))),
    CONSTRAINT request_logs_api_operation_format_check CHECK ((api_format='open_ai_chat_completions' AND api_operation='chat_completion')
                 OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search'))
                 OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit'))),
    CONSTRAINT request_logs_cache_write_tokens_check CHECK ((cache_write_tokens >= 0)),
    CONSTRAINT request_logs_cache_write_unit_price_check CHECK ((ag_decimal_cmp(cache_write_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_cached_input_tokens_check CHECK ((cached_input_tokens >= 0)),
    CONSTRAINT request_logs_cached_input_unit_price_check CHECK ((ag_decimal_cmp(cached_input_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_check CHECK ((completed_at >= started_at)),
    CONSTRAINT request_logs_check1 CHECK (((cached_input_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cached_input_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check2 CHECK (((cache_write_tokens IS NULL) OR ((input_tokens IS NOT NULL) AND (cache_write_tokens <= input_tokens)))),
    CONSTRAINT request_logs_check3 CHECK ((((currency IS NULL) AND (price_unit_tokens IS NULL) AND (price_effective_at IS NULL) AND (input_unit_price IS NULL) AND (cached_input_unit_price IS NULL) AND (cache_write_unit_price IS NULL) AND (output_unit_price IS NULL)) OR ((currency IS NOT NULL) AND (price_unit_tokens IS NOT NULL) AND (price_effective_at IS NOT NULL) AND (input_unit_price IS NOT NULL) AND (cached_input_unit_price IS NOT NULL) AND (cache_write_unit_price IS NOT NULL) AND (output_unit_price IS NOT NULL)))),
    CONSTRAINT request_logs_cost_amount_check CHECK ((ag_decimal_cmp(cost_amount, '0') >= 0)),
    CONSTRAINT request_logs_currency_usd_only CHECK (((currency IS NULL) OR (currency = 'USD'))),
    CONSTRAINT request_logs_failed_cancelled_zero_cost_check CHECK (((NOT ag_array_contains(json_array('failed', 'cancelled'), outcome)) OR ((cost_amount IS NOT NULL) AND (ag_decimal_cmp(cost_amount, '0') = 0)))),
    CONSTRAINT request_logs_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT request_logs_input_unit_price_check CHECK ((ag_decimal_cmp(input_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_outcome_check CHECK (ag_array_contains(json_array('succeeded', 'failed', 'rejected', 'cancelled'), outcome)),
    CONSTRAINT request_logs_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT request_logs_output_tokens_per_second_check CHECK ((ag_decimal_cmp(output_tokens_per_second, '0') >= 0)),
    CONSTRAINT request_logs_output_unit_price_check CHECK ((ag_decimal_cmp(output_unit_price, '0') >= 0)),
    CONSTRAINT request_logs_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT request_logs_reasoning_tokens_check CHECK ((reasoning_tokens >= 0)),
    CONSTRAINT request_logs_reasoning_tokens_within_output CHECK (((reasoning_tokens IS NULL) OR ((output_tokens IS NOT NULL) AND (reasoning_tokens <= output_tokens)))),
    CONSTRAINT request_logs_request_protocol_check CHECK (ag_array_contains(json_array('non_stream', 'sse', 'websocket'), request_protocol)),
    CONSTRAINT request_logs_request_source_check CHECK (ag_array_contains(json_array('client', 'scheduled_test'), request_source)),
    CONSTRAINT request_logs_response_status_code_check CHECK (((response_status_code >= 100) AND (response_status_code <= 599))),
    CONSTRAINT request_logs_total_duration_ms_check CHECK ((total_duration_ms >= 0)),
    CONSTRAINT request_logs_ttft_ms_check CHECK ((ttft_ms >= 0)),
    CONSTRAINT request_logs_pkey PRIMARY KEY (id),
    CONSTRAINT request_logs_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES api_keys (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES group_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES channel_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_model_id_fkey FOREIGN KEY (model_id) REFERENCES models (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_model_rule_id_fkey FOREIGN KEY (model_rule_id) REFERENCES model_rule_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "request_metering_facts" (
    id TEXT NOT NULL CONSTRAINT request_metering_facts_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    started_at TEXT NOT NULL CONSTRAINT request_metering_facts_started_at_storage CHECK (started_at IS NULL OR (ag_time_valid(started_at))) CHECK (started_at IS NULL OR instr(started_at, char(0))=0),
    completed_at TEXT NOT NULL CONSTRAINT request_metering_facts_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT request_metering_facts_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    api_key_id TEXT NOT NULL CONSTRAINT request_metering_facts_api_key_id_storage CHECK (api_key_id IS NULL OR (ag_uuid_valid(api_key_id))) CHECK (api_key_id IS NULL OR instr(api_key_id, char(0))=0),
    request_source TEXT NOT NULL CHECK (request_source IS NULL OR instr(request_source, char(0))=0),
    api_format TEXT COLLATE ag_api_format NOT NULL CONSTRAINT request_metering_facts_api_format_storage CHECK (api_format IS NULL OR (api_format IN ('open_ai_chat_completions','open_ai_responses','open_ai_images'))) CHECK (api_format IS NULL OR instr(api_format, char(0))=0),
    api_operation TEXT NOT NULL CHECK (api_operation IS NULL OR instr(api_operation, char(0))=0),
    request_protocol TEXT NOT NULL CHECK (request_protocol IS NULL OR instr(request_protocol, char(0))=0),
    client_model TEXT NOT NULL CONSTRAINT request_metering_facts_client_model_storage CHECK (client_model IS NULL OR (length(client_model) <= 300)) CHECK (client_model IS NULL OR instr(client_model, char(0))=0),
    upstream_model TEXT CONSTRAINT request_metering_facts_upstream_model_storage CHECK (upstream_model IS NULL OR (length(upstream_model) <= 300)) CHECK (upstream_model IS NULL OR instr(upstream_model, char(0))=0),
    model_rule_id TEXT CONSTRAINT request_metering_facts_model_rule_id_storage CHECK (model_rule_id IS NULL OR (ag_uuid_valid(model_rule_id))) CHECK (model_rule_id IS NULL OR instr(model_rule_id, char(0))=0),
    channel_group_id TEXT CONSTRAINT request_metering_facts_channel_group_id_storage CHECK (channel_group_id IS NULL OR (ag_uuid_valid(channel_group_id))) CHECK (channel_group_id IS NULL OR instr(channel_group_id, char(0))=0),
    channel_id TEXT CONSTRAINT request_metering_facts_channel_id_storage CHECK (channel_id IS NULL OR (ag_uuid_valid(channel_id))) CHECK (channel_id IS NULL OR instr(channel_id, char(0))=0),
    model_id TEXT CONSTRAINT request_metering_facts_model_id_storage CHECK (model_id IS NULL OR (ag_uuid_valid(model_id))) CHECK (model_id IS NULL OR instr(model_id, char(0))=0),
    outcome TEXT NOT NULL CHECK (outcome IS NULL OR instr(outcome, char(0))=0),
    input_tokens INTEGER,
    cached_input_tokens INTEGER,
    cache_write_tokens INTEGER,
    output_tokens INTEGER,
    reasoning_tokens INTEGER,
    currency TEXT CONSTRAINT request_metering_facts_currency_storage CHECK (currency IS NULL OR (length(currency) <= 3)) CHECK (currency IS NULL OR instr(currency, char(0))=0),
    price_unit_tokens INTEGER,
    price_effective_at TEXT CONSTRAINT request_metering_facts_price_effective_at_storage CHECK (price_effective_at IS NULL OR (ag_time_valid(price_effective_at))) CHECK (price_effective_at IS NULL OR instr(price_effective_at, char(0))=0),
    input_unit_price TEXT CONSTRAINT request_metering_facts_input_unit_price_storage CHECK (input_unit_price IS NULL OR (ag_decimal_valid(input_unit_price, 24, 12))) CHECK (input_unit_price IS NULL OR instr(input_unit_price, char(0))=0),
    cached_input_unit_price TEXT CONSTRAINT request_metering_facts_cached_input_unit_price_storage CHECK (cached_input_unit_price IS NULL OR (ag_decimal_valid(cached_input_unit_price, 24, 12))) CHECK (cached_input_unit_price IS NULL OR instr(cached_input_unit_price, char(0))=0),
    cache_write_unit_price TEXT CONSTRAINT request_metering_facts_cache_write_unit_price_storage CHECK (cache_write_unit_price IS NULL OR (ag_decimal_valid(cache_write_unit_price, 24, 12))) CHECK (cache_write_unit_price IS NULL OR instr(cache_write_unit_price, char(0))=0),
    output_unit_price TEXT CONSTRAINT request_metering_facts_output_unit_price_storage CHECK (output_unit_price IS NULL OR (ag_decimal_valid(output_unit_price, 24, 12))) CHECK (output_unit_price IS NULL OR instr(output_unit_price, char(0))=0),
    cost_amount TEXT CONSTRAINT request_metering_facts_cost_amount_storage CHECK (cost_amount IS NULL OR (ag_decimal_valid(cost_amount, 24, 8))) CHECK (cost_amount IS NULL OR instr(cost_amount, char(0))=0),
    peak_pricing INTEGER NOT NULL CONSTRAINT request_metering_facts_peak_pricing_storage CHECK (peak_pricing IS NULL OR (peak_pricing IN (0,1))),
    amount_state TEXT GENERATED ALWAYS AS (CASE WHEN ag_array_contains(json_array('failed', 'cancelled'), outcome) THEN 'zero_by_policy' WHEN ((cost_amount IS NULL) AND ((outcome = 'rejected') OR (api_operation='web_search'))) THEN 'not_applicable' WHEN (cost_amount IS NULL) THEN 'unknown' WHEN ((model_id IS NOT NULL) AND (currency IS NOT NULL)) THEN 'priced' ELSE 'invalid' END) STORED NOT NULL CHECK (amount_state IS NULL OR instr(amount_state, char(0))=0),
    upstream_credential_id TEXT CONSTRAINT request_metering_facts_upstream_credential_id_storage CHECK (upstream_credential_id IS NULL OR ag_uuid_valid(upstream_credential_id)),
    CONSTRAINT request_metering_facts_upstream_credential_id_fkey FOREIGN KEY (upstream_credential_id) REFERENCES upstream_credentials (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_cache_write_unit_price_check CHECK ((ag_decimal_cmp(cache_write_unit_price, '0') >= 0)),
    CONSTRAINT request_metering_facts_cached_input_unit_price_check CHECK ((ag_decimal_cmp(cached_input_unit_price, '0') >= 0)),
    CONSTRAINT request_metering_facts_check CHECK ((completed_at >= started_at)),
    CONSTRAINT request_metering_facts_check1 CHECK (((cached_input_tokens >= 0) AND (cached_input_tokens <= input_tokens))),
    CONSTRAINT request_metering_facts_check2 CHECK (((cache_write_tokens >= 0) AND (cache_write_tokens <= input_tokens))),
    CONSTRAINT request_metering_facts_check3 CHECK (((reasoning_tokens >= 0) AND (reasoning_tokens <= output_tokens))),
    CONSTRAINT request_metering_facts_check4 CHECK (((cached_input_tokens IS NULL) OR (input_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_check5 CHECK (((cache_write_tokens IS NULL) OR (input_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_check6 CHECK (((reasoning_tokens IS NULL) OR (output_tokens IS NOT NULL))),
    CONSTRAINT request_metering_facts_cost_amount_check CHECK ((ag_decimal_cmp(cost_amount, '0') >= 0)),
    CONSTRAINT request_metering_facts_currency_check CHECK ((currency = 'USD')),
    CONSTRAINT request_metering_facts_input_tokens_check CHECK ((input_tokens >= 0)),
    CONSTRAINT request_metering_facts_input_unit_price_check CHECK ((ag_decimal_cmp(input_unit_price, '0') >= 0)),
    CONSTRAINT request_metering_facts_outcome_check CHECK (ag_array_contains(json_array('succeeded', 'failed', 'rejected', 'cancelled'), outcome)),
    CONSTRAINT request_metering_facts_output_tokens_check CHECK ((output_tokens >= 0)),
    CONSTRAINT request_metering_facts_output_unit_price_check CHECK ((ag_decimal_cmp(output_unit_price, '0') >= 0)),
    CONSTRAINT request_metering_facts_price_unit_tokens_check CHECK ((price_unit_tokens > 0)),
    CONSTRAINT request_metering_facts_request_protocol_check CHECK (ag_array_contains(json_array('non_stream', 'sse', 'websocket'), request_protocol)),
    CONSTRAINT request_metering_facts_request_source_check CHECK (ag_array_contains(json_array('client', 'scheduled_test'), request_source)),
    CONSTRAINT request_metering_operation_format_check CHECK ((api_format='open_ai_chat_completions' AND api_operation='chat_completion')
                 OR (api_format='open_ai_responses' AND api_operation IN ('responses','responses-ws','web_search'))
                 OR (api_format='open_ai_images' AND api_operation IN ('images_generation','images_edit'))),
    CONSTRAINT request_metering_prices_check CHECK ((((currency IS NULL) AND (price_unit_tokens IS NULL) AND (price_effective_at IS NULL) AND (input_unit_price IS NULL) AND (cached_input_unit_price IS NULL) AND (cache_write_unit_price IS NULL) AND (output_unit_price IS NULL)) OR ((currency IS NOT NULL) AND (price_unit_tokens IS NOT NULL) AND (price_effective_at IS NOT NULL) AND (input_unit_price IS NOT NULL) AND (cached_input_unit_price IS NOT NULL) AND (cache_write_unit_price IS NOT NULL) AND (output_unit_price IS NOT NULL)))),
    CONSTRAINT request_metering_zero_policy_check CHECK (((NOT ag_array_contains(json_array('failed', 'cancelled'), outcome)) OR ((cost_amount IS NOT NULL) AND (ag_decimal_cmp(cost_amount, '0') = 0)))),
    CONSTRAINT request_metering_facts_pkey PRIMARY KEY (id),
    CONSTRAINT request_metering_facts_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES api_keys (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES group_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES channel_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_model_id_fkey FOREIGN KEY (model_id) REFERENCES models (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_model_rule_id_fkey FOREIGN KEY (model_rule_id) REFERENCES model_rule_identity_registry (id) ON DELETE RESTRICT,
    CONSTRAINT request_metering_facts_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE request_settlement_pending (
    request_id TEXT NOT NULL CONSTRAINT request_settlement_pending_request_id_storage CHECK (request_id IS NULL OR (ag_uuid_valid(request_id))) CHECK (request_id IS NULL OR instr(request_id, char(0))=0),
    completed_at TEXT NOT NULL CONSTRAINT request_settlement_pending_completed_at_storage CHECK (completed_at IS NULL OR (ag_time_valid(completed_at))) CHECK (completed_at IS NULL OR instr(completed_at, char(0))=0),
    CONSTRAINT request_settlement_pending_pkey PRIMARY KEY (request_id),
    CONSTRAINT request_settlement_pending_request_id_fkey FOREIGN KEY (request_id) REFERENCES request_metering_facts (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE request_settlements (
    request_id TEXT NOT NULL CONSTRAINT request_settlements_request_id_storage CHECK (request_id IS NULL OR (ag_uuid_valid(request_id))) CHECK (request_id IS NULL OR instr(request_id, char(0))=0),
    cost_amount TEXT NOT NULL CONSTRAINT request_settlements_cost_amount_storage CHECK (cost_amount IS NULL OR (ag_decimal_valid(cost_amount, 24, 8))) CHECK (cost_amount IS NULL OR instr(cost_amount, char(0))=0),
    currency TEXT CONSTRAINT request_settlements_currency_storage CHECK (currency IS NULL OR (length(currency) <= 3)) CHECK (currency IS NULL OR instr(currency, char(0))=0),
    settled_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT request_settlements_settled_at_storage CHECK (settled_at IS NULL OR (ag_time_valid(settled_at))) CHECK (settled_at IS NULL OR instr(settled_at, char(0))=0),
    policy_version INTEGER DEFAULT (1) NOT NULL CONSTRAINT request_settlements_policy_version_storage CHECK (policy_version IS NULL OR (policy_version BETWEEN -32768 AND 32767)),
    CONSTRAINT request_settlements_cost_amount_check CHECK ((ag_decimal_cmp(cost_amount, '0') >= 0)),
    CONSTRAINT request_settlements_policy_version_check CHECK ((policy_version = 1)),
    CONSTRAINT request_settlements_pkey PRIMARY KEY (request_id),
    CONSTRAINT request_settlements_request_id_fkey FOREIGN KEY (request_id) REFERENCES request_metering_facts (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE routing_groups (
    id TEXT NOT NULL CONSTRAINT routing_groups_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT routing_groups_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT routing_groups_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT routing_groups_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT routing_groups_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT routing_groups_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT routing_groups_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT routing_groups_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT routing_groups_pkey PRIMARY KEY (id)
) STRICT;

CREATE TABLE spend_leaderboard_entries (
    period TEXT NOT NULL CHECK (period IS NULL OR instr(period, char(0))=0),
    period_start TEXT NOT NULL CONSTRAINT spend_leaderboard_entries_period_start_storage CHECK (period_start IS NULL OR (ag_date_valid(period_start))) CHECK (period_start IS NULL OR instr(period_start, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT spend_leaderboard_entries_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    rank INTEGER NOT NULL,
    request_count INTEGER NOT NULL,
    priced_request_count INTEGER NOT NULL,
    total_tokens INTEGER NOT NULL,
    cost_amount TEXT NOT NULL CONSTRAINT spend_leaderboard_entries_cost_amount_storage CHECK (cost_amount IS NULL OR (ag_decimal_valid(cost_amount, 24, 8))) CHECK (cost_amount IS NULL OR instr(cost_amount, char(0))=0),
    CONSTRAINT spend_leaderboard_entries_check CHECK (((priced_request_count >= 0) AND (priced_request_count <= request_count))),
    CONSTRAINT spend_leaderboard_entries_cost_amount_check CHECK ((ag_decimal_cmp(cost_amount, '0') >= 0)),
    CONSTRAINT spend_leaderboard_entries_rank_check CHECK ((rank > 0)),
    CONSTRAINT spend_leaderboard_entries_request_count_check CHECK ((request_count >= 0)),
    CONSTRAINT spend_leaderboard_entries_total_tokens_check CHECK ((total_tokens >= 0)),
    CONSTRAINT spend_leaderboard_entries_pkey PRIMARY KEY (period, period_start, user_id),
    CONSTRAINT spend_leaderboard_entries_period_period_start_fkey FOREIGN KEY (period, period_start) REFERENCES spend_leaderboard_periods (period, period_start) ON DELETE CASCADE,
    CONSTRAINT spend_leaderboard_entries_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE spend_leaderboard_periods (
    period TEXT NOT NULL CHECK (period IS NULL OR instr(period, char(0))=0),
    period_start TEXT NOT NULL CONSTRAINT spend_leaderboard_periods_period_start_storage CHECK (period_start IS NULL OR (ag_date_valid(period_start))) CHECK (period_start IS NULL OR instr(period_start, char(0))=0),
    period_end TEXT NOT NULL CONSTRAINT spend_leaderboard_periods_period_end_storage CHECK (period_end IS NULL OR (ag_date_valid(period_end))) CHECK (period_end IS NULL OR instr(period_end, char(0))=0),
    refreshed_at TEXT NOT NULL CONSTRAINT spend_leaderboard_periods_refreshed_at_storage CHECK (refreshed_at IS NULL OR (ag_time_valid(refreshed_at))) CHECK (refreshed_at IS NULL OR instr(refreshed_at, char(0))=0),
    total_cost_amount TEXT NOT NULL CONSTRAINT spend_leaderboard_periods_total_cost_amount_storage CHECK (total_cost_amount IS NULL OR (ag_decimal_valid(total_cost_amount, 24, 8))) CHECK (total_cost_amount IS NULL OR instr(total_cost_amount, char(0))=0),
    CONSTRAINT spend_leaderboard_periods_check CHECK ((period_end > period_start)),
    CONSTRAINT spend_leaderboard_periods_period_check CHECK (ag_array_contains(json_array('day', 'week', 'month'), period)),
    CONSTRAINT spend_leaderboard_periods_total_cost_amount_check CHECK ((ag_decimal_cmp(total_cost_amount, '0') >= 0)),
    CONSTRAINT spend_leaderboard_periods_pkey PRIMARY KEY (period, period_start)
) STRICT;

CREATE TABLE system_settings (
    setting_key TEXT NOT NULL CONSTRAINT system_settings_setting_key_storage CHECK (setting_key IS NULL OR (length(setting_key) <= 100)) CHECK (setting_key IS NULL OR instr(setting_key, char(0))=0),
    value TEXT NOT NULL CONSTRAINT system_settings_value_storage CHECK (value IS NULL OR (ag_json_valid(value))) CHECK (value IS NULL OR instr(value, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT system_settings_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    CONSTRAINT system_settings_value_check CHECK ((json_type(value) = 'object')),
    CONSTRAINT system_settings_pkey PRIMARY KEY (setting_key)
) STRICT;

CREATE TABLE "upstream_accesses" (
    id TEXT NOT NULL CONSTRAINT upstream_accesses_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT upstream_accesses_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    connector_kind TEXT NOT NULL CONSTRAINT upstream_accesses_connector_kind_storage CHECK (connector_kind IS NULL OR instr(connector_kind, char(0))=0),
    base_url TEXT NOT NULL CONSTRAINT upstream_accesses_base_url_storage CHECK (base_url IS NULL OR instr(base_url, char(0))=0),
    proxy_id TEXT CONSTRAINT upstream_accesses_proxy_id_storage CHECK (proxy_id IS NULL OR (ag_uuid_valid(proxy_id) AND instr(proxy_id, char(0))=0)),
    connect_timeout_ms INTEGER CONSTRAINT upstream_accesses_connect_timeout_ms_storage CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms BETWEEN -2147483648 AND 2147483647),
    response_header_timeout_ms INTEGER CONSTRAINT upstream_accesses_response_header_timeout_ms_storage CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms BETWEEN -2147483648 AND 2147483647),
    stream_idle_timeout_ms INTEGER CONSTRAINT upstream_accesses_stream_idle_timeout_ms_storage CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms BETWEEN -2147483648 AND 2147483647),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT upstream_accesses_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT upstream_accesses_revision_storage CHECK (revision IS NULL OR (ag_uuid_valid(revision) AND instr(revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_accesses_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_accesses_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT upstream_accesses_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT upstream_accesses_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_accesses_connector_kind_check CHECK (length(connector_kind) BETWEEN 1 AND 64 AND connector_kind GLOB '[a-z]*' AND connector_kind NOT GLOB '*[^a-z0-9_-]*'),
    CONSTRAINT upstream_accesses_base_url_check
        CHECK (ag_regex('^https?://', base_url)),
    CONSTRAINT upstream_accesses_connect_timeout_check
        CHECK (connect_timeout_ms IS NULL OR connect_timeout_ms > 0),
    CONSTRAINT upstream_accesses_response_header_timeout_check
        CHECK (response_header_timeout_ms IS NULL OR response_header_timeout_ms > 0),
    CONSTRAINT upstream_accesses_stream_idle_timeout_check
        CHECK (stream_idle_timeout_ms IS NULL OR stream_idle_timeout_ms > 0),
    CONSTRAINT upstream_accesses_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT upstream_accesses_pkey PRIMARY KEY (id),
    CONSTRAINT upstream_accesses_proxy_id_fkey FOREIGN KEY (proxy_id) REFERENCES proxies (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE upstream_channels (
    id TEXT NOT NULL CONSTRAINT upstream_channels_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id) AND instr(id, char(0))=0)),
    group_id TEXT NOT NULL CONSTRAINT upstream_channels_group_id_storage CHECK (group_id IS NULL OR (ag_uuid_valid(group_id) AND instr(group_id, char(0))=0)),
    access_id TEXT NOT NULL CONSTRAINT upstream_channels_access_id_storage CHECK (access_id IS NULL OR (ag_uuid_valid(access_id) AND instr(access_id, char(0))=0)),
    credential_id TEXT CONSTRAINT upstream_channels_credential_id_storage CHECK (credential_id IS NULL OR (ag_uuid_valid(credential_id) AND instr(credential_id, char(0))=0)),
    name TEXT NOT NULL CONSTRAINT upstream_channels_name_storage CHECK (name IS NULL OR (length(name) <= 100 AND instr(name, char(0))=0)),
    enabled INTEGER DEFAULT 1 NOT NULL CONSTRAINT upstream_channels_enabled_storage CHECK (enabled IS NULL OR enabled IN (0,1)),
    binding_revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CONSTRAINT upstream_channels_binding_revision_storage CHECK (binding_revision IS NULL OR (ag_uuid_valid(binding_revision) AND instr(binding_revision, char(0))=0)),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_channels_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at) AND instr(created_at, char(0))=0)),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT upstream_channels_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at) AND instr(updated_at, char(0))=0)),
    deleted_at TEXT CONSTRAINT upstream_channels_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at) AND instr(deleted_at, char(0))=0)),
    CONSTRAINT upstream_channels_name_check
        CHECK (length(trim(name)) BETWEEN 1 AND 100),
    CONSTRAINT upstream_channels_deleted_state_check
        CHECK (deleted_at IS NULL OR NOT enabled),
    CONSTRAINT upstream_channels_pkey PRIMARY KEY (id),
    CONSTRAINT upstream_channels_group_id_fkey FOREIGN KEY (group_id) REFERENCES routing_groups (id) ON DELETE RESTRICT,
    CONSTRAINT upstream_channels_access_id_fkey FOREIGN KEY (access_id) REFERENCES upstream_accesses (id) ON DELETE RESTRICT,
    CONSTRAINT upstream_channels_credential_id_fkey FOREIGN KEY (credential_id) REFERENCES upstream_credentials (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE "upstream_credentials" (
    id TEXT CONSTRAINT upstream_credentials_pkey PRIMARY KEY NOT NULL CHECK (ag_uuid_valid(id)),
    name TEXT NOT NULL CONSTRAINT upstream_credentials_name_check CHECK (length(trim(name)) BETWEEN 1 AND 100),
    kind TEXT NOT NULL CONSTRAINT upstream_credentials_kind_check CHECK (kind IN ('bearer','header','codex_oauth')),
    header_name TEXT,
    secret TEXT,
    allowed_base_urls TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(allowed_base_urls)),
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
    revision TEXT NOT NULL DEFAULT (ag_md5_uuid(hex(randomblob(32)))) CHECK (ag_uuid_valid(revision)),
    created_at TEXT NOT NULL DEFAULT (ag_now()) CHECK (ag_time_valid(created_at)),
    updated_at TEXT NOT NULL DEFAULT (ag_now()) CHECK (ag_time_valid(updated_at)),
    deleted_at TEXT CHECK (deleted_at IS NULL OR ag_time_valid(deleted_at)), connector_kind TEXT NOT NULL DEFAULT 'general' CONSTRAINT upstream_credentials_connector_kind_check CHECK (length(connector_kind) BETWEEN 1 AND 64 AND connector_kind GLOB '[a-z]*' AND connector_kind NOT GLOB '*[^a-z0-9_-]*' AND ((kind='codex_oauth')=(connector_kind='codex'))),
    CONSTRAINT upstream_credentials_scope_check CHECK (json_type(allowed_base_urls)='array'),
    CONSTRAINT upstream_credentials_auth_check CHECK (
        (deleted_at IS NOT NULL AND secret IS NULL AND NOT enabled)
        OR (deleted_at IS NULL AND (
            (kind='codex_oauth' AND secret IS NULL AND header_name IS NULL AND json_array_length(allowed_base_urls)>0)
            OR (kind='bearer' AND secret IS NOT NULL AND length(trim(secret))>0
                AND header_name IS NULL AND json_array_length(allowed_base_urls)>0)
            OR (kind='header' AND secret IS NOT NULL AND length(trim(secret))>0
                AND header_name IS NOT NULL AND json_array_length(allowed_base_urls)>0)
        ))
    )
) STRICT;

CREATE TABLE "user_group_codex_quota_visibility" (
    user_group_id TEXT NOT NULL CONSTRAINT user_group_codex_quota_visibility_user_group_id_storage CHECK (user_group_id IS NULL OR (ag_uuid_valid(user_group_id))) CHECK (user_group_id IS NULL OR instr(user_group_id, char(0))=0),
    channel_group_id TEXT NOT NULL CONSTRAINT user_group_codex_quota_visibility_channel_group_id_storage CHECK (channel_group_id IS NULL OR (ag_uuid_valid(channel_group_id))) CHECK (channel_group_id IS NULL OR instr(channel_group_id, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_group_codex_quota_visibility_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    CONSTRAINT user_group_codex_quota_visibility_pkey PRIMARY KEY (user_group_id, channel_group_id),
    CONSTRAINT user_group_codex_quota_visibility_channel_group_id_fkey FOREIGN KEY (channel_group_id) REFERENCES routing_groups (id) ON DELETE RESTRICT,
    CONSTRAINT user_group_codex_quota_visibility_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES user_groups (id) ON DELETE CASCADE
) STRICT;

CREATE TABLE user_groups (
    id TEXT NOT NULL CONSTRAINT user_groups_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    name TEXT NOT NULL CONSTRAINT user_groups_name_storage CHECK (name IS NULL OR (length(name) <= 100)) CHECK (name IS NULL OR instr(name, char(0))=0),
    description TEXT CONSTRAINT user_groups_description_storage CHECK (description IS NULL OR (length(description) <= 500)) CHECK (description IS NULL OR instr(description, char(0))=0),
    default_api_key_policy_id TEXT CONSTRAINT user_groups_default_api_key_policy_id_storage CHECK (default_api_key_policy_id IS NULL OR (ag_uuid_valid(default_api_key_policy_id))) CHECK (default_api_key_policy_id IS NULL OR instr(default_api_key_policy_id, char(0))=0),
    system_role TEXT CHECK (system_role IS NULL OR instr(system_role, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_groups_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_groups_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    filter_fast_mode INTEGER DEFAULT (0) NOT NULL CONSTRAINT user_groups_filter_fast_mode_storage CHECK (filter_fast_mode IS NULL OR (filter_fast_mode IN (0,1))),
    deleted_at TEXT CONSTRAINT user_groups_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    deleted_by TEXT CONSTRAINT user_groups_deleted_by_storage CHECK (deleted_by IS NULL OR (ag_uuid_valid(deleted_by))) CHECK (deleted_by IS NULL OR instr(deleted_by, char(0))=0),
    CONSTRAINT user_groups_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT user_groups_system_role_check CHECK (((system_role IS NULL) OR ag_array_contains(json_array('user', 'admin'), system_role))),
    CONSTRAINT user_groups_pkey PRIMARY KEY (id),
    CONSTRAINT user_groups_system_role_key UNIQUE (system_role),
    CONSTRAINT user_groups_default_api_key_policy_id_fkey FOREIGN KEY (default_api_key_policy_id) REFERENCES api_key_policies (id) ON DELETE RESTRICT,
    CONSTRAINT user_groups_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE user_invitations (
    id TEXT NOT NULL CONSTRAINT user_invitations_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT user_invitations_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    invited_by TEXT NOT NULL CONSTRAINT user_invitations_invited_by_storage CHECK (invited_by IS NULL OR (ag_uuid_valid(invited_by))) CHECK (invited_by IS NULL OR instr(invited_by, char(0))=0),
    token_hash BLOB NOT NULL,
    expires_at TEXT NOT NULL CONSTRAINT user_invitations_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    accepted_at TEXT CONSTRAINT user_invitations_accepted_at_storage CHECK (accepted_at IS NULL OR (ag_time_valid(accepted_at))) CHECK (accepted_at IS NULL OR instr(accepted_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_invitations_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    revoked_at TEXT CONSTRAINT user_invitations_revoked_at_storage CHECK (revoked_at IS NULL OR (ag_time_valid(revoked_at))) CHECK (revoked_at IS NULL OR instr(revoked_at, char(0))=0),
    CONSTRAINT user_invitations_check CHECK ((expires_at > created_at)),
    CONSTRAINT user_invitations_pkey PRIMARY KEY (id),
    CONSTRAINT user_invitations_invited_by_fkey FOREIGN KEY (invited_by) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT user_invitations_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE user_sessions (
    id TEXT NOT NULL CONSTRAINT user_sessions_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    user_id TEXT NOT NULL CONSTRAINT user_sessions_user_id_storage CHECK (user_id IS NULL OR (ag_uuid_valid(user_id))) CHECK (user_id IS NULL OR instr(user_id, char(0))=0),
    refresh_token_hash BLOB NOT NULL,
    expires_at TEXT NOT NULL CONSTRAINT user_sessions_expires_at_storage CHECK (expires_at IS NULL OR (ag_time_valid(expires_at))) CHECK (expires_at IS NULL OR instr(expires_at, char(0))=0),
    revoked_at TEXT CONSTRAINT user_sessions_revoked_at_storage CHECK (revoked_at IS NULL OR (ag_time_valid(revoked_at))) CHECK (revoked_at IS NULL OR instr(revoked_at, char(0))=0),
    rotated_at TEXT CONSTRAINT user_sessions_rotated_at_storage CHECK (rotated_at IS NULL OR (ag_time_valid(rotated_at))) CHECK (rotated_at IS NULL OR instr(rotated_at, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_sessions_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    last_seen_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT user_sessions_last_seen_at_storage CHECK (last_seen_at IS NULL OR (ag_time_valid(last_seen_at))) CHECK (last_seen_at IS NULL OR instr(last_seen_at, char(0))=0),
    user_agent TEXT CONSTRAINT user_sessions_user_agent_storage CHECK (user_agent IS NULL OR (length(user_agent) <= 512)) CHECK (user_agent IS NULL OR instr(user_agent, char(0))=0),
    purpose TEXT DEFAULT ('normal') NOT NULL CHECK (purpose IS NULL OR instr(purpose, char(0))=0),
    CONSTRAINT user_sessions_check CHECK ((expires_at > created_at)),
    CONSTRAINT user_sessions_purpose_check CHECK (ag_array_contains(json_array('normal', 'password_change'), purpose)),
    CONSTRAINT user_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE users (
    id TEXT NOT NULL CONSTRAINT users_id_storage CHECK (id IS NULL OR (ag_uuid_valid(id))) CHECK (id IS NULL OR instr(id, char(0))=0),
    display_name TEXT NOT NULL CONSTRAINT users_display_name_storage CHECK (display_name IS NULL OR (length(display_name) <= 200)) CHECK (display_name IS NULL OR instr(display_name, char(0))=0),
    status TEXT DEFAULT ('active') NOT NULL CHECK (status IS NULL OR instr(status, char(0))=0),
    balance_amount TEXT DEFAULT ('0') NOT NULL CONSTRAINT users_balance_amount_storage CHECK (balance_amount IS NULL OR (ag_decimal_valid(balance_amount, 24, 8))) CHECK (balance_amount IS NULL OR instr(balance_amount, char(0))=0),
    created_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT users_created_at_storage CHECK (created_at IS NULL OR (ag_time_valid(created_at))) CHECK (created_at IS NULL OR instr(created_at, char(0))=0),
    updated_at TEXT DEFAULT (ag_now()) NOT NULL CONSTRAINT users_updated_at_storage CHECK (updated_at IS NULL OR (ag_time_valid(updated_at))) CHECK (updated_at IS NULL OR instr(updated_at, char(0))=0),
    email TEXT CONSTRAINT users_email_storage CHECK (email IS NULL OR (length(email) <= 320)) CHECK (email IS NULL OR instr(email, char(0))=0),
    role TEXT DEFAULT ('user') NOT NULL CHECK (role IS NULL OR instr(role, char(0))=0),
    password_hash TEXT CHECK (password_hash IS NULL OR instr(password_hash, char(0))=0),
    auth_version INTEGER DEFAULT (1) NOT NULL,
    password_changed_at TEXT CONSTRAINT users_password_changed_at_storage CHECK (password_changed_at IS NULL OR (ag_time_valid(password_changed_at))) CHECK (password_changed_at IS NULL OR instr(password_changed_at, char(0))=0),
    default_api_key_policy_id TEXT CONSTRAINT users_default_api_key_policy_id_storage CHECK (default_api_key_policy_id IS NULL OR (ag_uuid_valid(default_api_key_policy_id))) CHECK (default_api_key_policy_id IS NULL OR instr(default_api_key_policy_id, char(0))=0),
    is_system INTEGER DEFAULT (0) NOT NULL CONSTRAINT users_is_system_storage CHECK (is_system IS NULL OR (is_system IN (0,1))),
    user_group_id TEXT DEFAULT ('00000000-0000-0000-0000-000000000101') NOT NULL CONSTRAINT users_user_group_id_storage CHECK (user_group_id IS NULL OR (ag_uuid_valid(user_group_id))) CHECK (user_group_id IS NULL OR instr(user_group_id, char(0))=0),
    deleted_at TEXT CONSTRAINT users_deleted_at_storage CHECK (deleted_at IS NULL OR (ag_time_valid(deleted_at))) CHECK (deleted_at IS NULL OR instr(deleted_at, char(0))=0),
    deleted_by TEXT CONSTRAINT users_deleted_by_storage CHECK (deleted_by IS NULL OR (ag_uuid_valid(deleted_by))) CHECK (deleted_by IS NULL OR instr(deleted_by, char(0))=0),
    websocket_enabled INTEGER DEFAULT (0) NOT NULL CONSTRAINT users_websocket_enabled_storage CHECK (websocket_enabled IS NULL OR (websocket_enabled IN (0,1))),
    password_change_required INTEGER DEFAULT (0) NOT NULL CONSTRAINT users_password_change_required_storage CHECK (password_change_required IS NULL OR (password_change_required IN (0,1))),
    temporary_password_issued_at TEXT CONSTRAINT users_temporary_password_issued_at_storage CHECK (temporary_password_issued_at IS NULL OR (ag_time_valid(temporary_password_issued_at))) CHECK (temporary_password_issued_at IS NULL OR instr(temporary_password_issued_at, char(0))=0),
    temporary_password_expires_at TEXT CONSTRAINT users_temporary_password_expires_at_storage CHECK (temporary_password_expires_at IS NULL OR (ag_time_valid(temporary_password_expires_at))) CHECK (temporary_password_expires_at IS NULL OR instr(temporary_password_expires_at, char(0))=0),
    CONSTRAINT users_auth_version_check CHECK ((auth_version > 0)),
    CONSTRAINT users_deleted_actor_check CHECK (((deleted_at IS NULL) = (deleted_by IS NULL))),
    CONSTRAINT users_role_check CHECK (ag_array_contains(json_array('user', 'admin'), role)),
    CONSTRAINT users_status_check CHECK (ag_array_contains(json_array('invited', 'active', 'suspended', 'disabled'), status)),
    CONSTRAINT users_temporary_password_state_check CHECK (((password_change_required AND (password_hash IS NOT NULL) AND (temporary_password_issued_at IS NOT NULL) AND (temporary_password_expires_at IS NOT NULL) AND (temporary_password_expires_at > temporary_password_issued_at)) OR ((NOT password_change_required) AND (temporary_password_issued_at IS NULL) AND (temporary_password_expires_at IS NULL)))),
    CONSTRAINT users_name_key UNIQUE (display_name),
    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_default_api_key_policy_id_fkey FOREIGN KEY (default_api_key_policy_id) REFERENCES api_key_policies (id) ON DELETE RESTRICT,
    CONSTRAINT users_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES users (id) ON DELETE RESTRICT,
    CONSTRAINT users_user_group_id_fkey FOREIGN KEY (user_group_id) REFERENCES user_groups (id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX api_key_channel_grants_channel_id_idx ON api_key_channel_grants(channel_id);

CREATE INDEX api_key_policy_channel_grants_channel_id_idx ON api_key_policy_channel_grants(channel_id);

CREATE UNIQUE INDEX api_keys_active_user_name_idx ON api_keys (user_id, name) WHERE (deleted_at IS NULL);

CREATE INDEX api_keys_deleted_at_idx ON api_keys (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX api_keys_user_id_status_idx ON api_keys (user_id, status);

CREATE INDEX audit_logs_actor_occurred_at_idx ON audit_logs (actor_user_id, occurred_at DESC);

CREATE INDEX audit_logs_object_occurred_at_idx ON audit_logs (object_type, object_id, occurred_at DESC);

CREATE INDEX channel_capabilities_deleted_at_idx
    ON channel_capabilities (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX codex_credentials_account_identity_idx
    ON codex_oauth_credentials(account_id,user_id) WHERE deleted_at IS NULL;

CREATE INDEX codex_oauth_credentials_quota_idx ON codex_oauth_credentials (runtime_status, quota_checked_at);

CREATE INDEX codex_oauth_credentials_refresh_idx ON codex_oauth_credentials (runtime_status, access_token_expires_at);

CREATE INDEX codex_oauth_flows_expiry_idx ON codex_oauth_flows (expires_at) WHERE (completed_at IS NULL);

CREATE INDEX codex_quota_reset_events_pending_idx ON codex_quota_reset_events (credential_id, requested_at DESC) WHERE (ag_array_contains(json_array('reset', 'already_redeemed'), outcome) AND (windows_reset > ((CASE WHEN (primary_applied_at IS NULL) THEN 0 ELSE 1 END) + (CASE WHEN (secondary_applied_at IS NULL) THEN 0 ELSE 1 END))));

CREATE UNIQUE INDEX codex_quota_window_periods_current_idx ON codex_quota_window_periods (credential_id, window_kind) WHERE (ended_at IS NULL);

CREATE INDEX codex_quota_window_periods_history_idx ON codex_quota_window_periods (credential_id, window_kind, started_at DESC);

CREATE UNIQUE INDEX codex_quota_window_periods_identity_idx ON codex_quota_window_periods (credential_id, window_kind, started_at, scheduled_reset_at);

CREATE INDEX model_capability_candidates_capability_id_idx
    ON model_capability_candidates (capability_id);

CREATE UNIQUE INDEX models_active_source_model_id_idx ON models (source_model_id) WHERE (deleted_at IS NULL);

CREATE INDEX models_deleted_at_idx ON models (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX plugin_install_jobs_status_idx ON plugin_install_jobs(status, created_at);

CREATE INDEX registration_invitation_codes_user_group_id_idx ON registration_invitation_codes (user_group_id);

CREATE INDEX request_log_ingest_metering_idx ON request_log_ingest (sequence) WHERE (metered_at IS NULL);

CREATE INDEX request_log_ingest_metering_retry_idx ON request_log_ingest (metering_next_attempt_at, sequence) WHERE ((metered_at IS NULL) AND (metering_attempt_count > 0));

CREATE INDEX request_log_ingest_projection_idx ON request_log_ingest (sequence) WHERE (metered_at IS NOT NULL);

CREATE INDEX request_log_ingest_retry_idx ON request_log_ingest (next_attempt_at, sequence) WHERE (attempt_count > 0);

CREATE INDEX request_logs_api_format_started_at_idx ON request_logs (api_format, started_at DESC, id DESC);

CREATE INDEX request_logs_api_key_id_started_at_idx ON request_logs (api_key_id, started_at DESC);

CREATE INDEX request_logs_channel_group_model_started_at_idx ON request_logs (channel_group_id, upstream_model, started_at DESC) WHERE (channel_group_id IS NOT NULL);

CREATE INDEX request_logs_channel_id_started_at_idx ON request_logs (channel_id, started_at DESC);

CREATE INDEX request_logs_channel_model_started_at_idx ON request_logs (channel_id, upstream_model, started_at DESC) WHERE (channel_id IS NOT NULL);

CREATE INDEX request_logs_client_model_started_at_idx ON request_logs (client_model, started_at DESC, id DESC);

CREATE INDEX request_logs_failed_started_at_idx ON request_logs (started_at DESC) WHERE (outcome = 'failed');

CREATE INDEX request_logs_outcome_started_at_idx ON request_logs (outcome, started_at DESC, id DESC);

CREATE INDEX request_logs_scheduled_test_started_at_idx ON request_logs (started_at DESC, id DESC) WHERE (request_source = 'scheduled_test');

CREATE INDEX request_logs_started_at_id_idx ON request_logs (started_at DESC, id DESC);

CREATE INDEX request_logs_upstream_model_started_at_idx ON request_logs (upstream_model, started_at DESC, id DESC);

CREATE INDEX request_logs_user_id_started_at_idx ON request_logs (user_id, started_at DESC);

CREATE INDEX request_metering_channel_time_idx ON request_metering_facts (channel_id, started_at);

CREATE INDEX request_metering_key_time_idx ON request_metering_facts (api_key_id, started_at);

CREATE INDEX request_metering_reconciliation_idx ON request_metering_facts (amount_state, id) WHERE ag_array_contains(json_array('unknown', 'invalid'), amount_state);

CREATE INDEX request_metering_time_idx ON request_metering_facts (started_at);

CREATE INDEX request_metering_user_time_idx ON request_metering_facts (user_id, started_at);

CREATE INDEX request_settlement_pending_order_idx ON request_settlement_pending (completed_at, request_id);

CREATE UNIQUE INDEX routing_groups_active_name_idx
    ON routing_groups (name)
    WHERE deleted_at IS NULL;

CREATE INDEX routing_groups_deleted_at_idx
    ON routing_groups (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX spend_leaderboard_entries_period_rank_idx ON spend_leaderboard_entries (period, period_start, rank);

CREATE INDEX upstream_accesses_active_name_idx
    ON upstream_accesses (name)
    WHERE deleted_at IS NULL;

CREATE INDEX upstream_accesses_deleted_at_idx
    ON upstream_accesses (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX upstream_channels_access_id_idx
    ON upstream_channels (access_id);

CREATE UNIQUE INDEX upstream_channels_active_group_name_idx
    ON upstream_channels (group_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX upstream_channels_credential_id_idx
    ON upstream_channels (credential_id)
    WHERE credential_id IS NOT NULL;

CREATE INDEX upstream_channels_deleted_at_idx
    ON upstream_channels (deleted_at)
    WHERE deleted_at IS NOT NULL;

CREATE INDEX user_group_codex_quota_visibility_channel_group_idx ON user_group_codex_quota_visibility (channel_group_id);

CREATE UNIQUE INDEX user_groups_active_name_idx ON user_groups (name) WHERE (deleted_at IS NULL);

CREATE INDEX user_groups_default_api_key_policy_id_idx ON user_groups (default_api_key_policy_id);

CREATE INDEX user_groups_deleted_at_idx ON user_groups (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE INDEX user_invitations_user_id_active_idx ON user_invitations (user_id, expires_at DESC) WHERE ((accepted_at IS NULL) AND (revoked_at IS NULL));

CREATE INDEX user_sessions_user_id_active_idx ON user_sessions (user_id, expires_at DESC) WHERE (revoked_at IS NULL);

CREATE INDEX users_default_api_key_policy_id_idx ON users (default_api_key_policy_id);

CREATE INDEX users_deleted_at_idx ON users (deleted_at) WHERE (deleted_at IS NOT NULL);

CREATE UNIQUE INDEX users_email_lower_unique_idx ON users (ag_lower(email)) WHERE (email IS NOT NULL);

CREATE INDEX users_user_group_id_idx ON users (user_group_id) WHERE (deleted_at IS NULL);

CREATE TRIGGER api_key_policies_timestamp BEFORE UPDATE ON api_key_policies
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'api_key_policies_timestamp');
END;

CREATE TRIGGER api_keys_no_hard_delete BEFORE DELETE ON api_keys BEGIN
    SELECT RAISE(ABORT, 'api_keys_no_hard_delete');
END;

CREATE TRIGGER api_keys_timestamp BEFORE UPDATE ON api_keys
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'api_keys_timestamp');
END;

CREATE TRIGGER audit_logs_immutable_delete BEFORE DELETE ON audit_logs BEGIN
    SELECT RAISE(ABORT, 'audit_logs_immutable_delete');
END;

CREATE TRIGGER audit_logs_immutable_update BEFORE UPDATE ON audit_logs BEGIN
    SELECT RAISE(ABORT, 'audit_logs_immutable_update');
END;

CREATE TRIGGER canonical_access_codex_operation_fence BEFORE UPDATE ON upstream_accesses
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.access_id=OLD.id AND c.deleted_at IS NULL
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_capability_codex_operation_insert_fence BEFORE INSERT ON channel_capabilities
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.id=NEW.channel_id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_capability_codex_operation_update_fence BEFORE UPDATE ON channel_capabilities
WHEN EXISTS (
    SELECT 1 FROM upstream_channels c
    JOIN _gateway_codex_operations pending ON pending.credential_id=c.credential_id
    WHERE c.id IN (OLD.channel_id,NEW.channel_id)
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_channel_codex_operation_insert_fence BEFORE INSERT ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id=NEW.credential_id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER canonical_channel_codex_operation_update_fence BEFORE UPDATE ON upstream_channels
WHEN EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id IN (OLD.credential_id,NEW.credential_id)
)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

CREATE TRIGGER canonical_credential_codex_operation_fence BEFORE UPDATE ON upstream_credentials
WHEN OLD.kind='codex_oauth' AND EXISTS (
    SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.id
) BEGIN
    SELECT RAISE(ABORT, 'codex_operation_pending');
END;

CREATE TRIGGER capabilities_active_probe_insert BEFORE INSERT ON channel_capabilities
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'capabilities_active_probe');
END;

CREATE TRIGGER capabilities_active_probe_update BEFORE UPDATE OF test_pricing_model_id ON channel_capabilities
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.test_pricing_model_id AND deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'capabilities_active_probe');
END;

CREATE TRIGGER channel_capabilities_preserve_tombstones_delete BEFORE DELETE ON channel_capabilities BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_preserve_tombstones');
END;

CREATE TRIGGER channel_capabilities_preserve_tombstones_update BEFORE UPDATE ON channel_capabilities
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_preserve_tombstones');
END;

CREATE TRIGGER channel_capabilities_timestamp BEFORE UPDATE ON channel_capabilities
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'channel_capabilities_timestamp');
END;

CREATE TRIGGER channel_identity_registry_immutable_delete BEFORE DELETE ON channel_identity_registry BEGIN
    SELECT RAISE(ABORT, 'channel_identity_registry_immutable');
END;

CREATE TRIGGER channel_identity_registry_immutable_update BEFORE UPDATE ON channel_identity_registry BEGIN
    SELECT RAISE(ABORT, 'channel_identity_registry_immutable');
END;

CREATE TRIGGER codex_oauth_credentials_timestamp BEFORE UPDATE ON codex_oauth_credentials
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'codex_oauth_credentials_timestamp');
END;

CREATE TRIGGER codex_operation_delete_fence BEFORE DELETE ON codex_oauth_credentials
WHEN EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.channel_id)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

CREATE TRIGGER codex_operation_update_fence BEFORE UPDATE ON codex_oauth_credentials
WHEN EXISTS(SELECT 1 FROM _gateway_codex_operations WHERE credential_id=OLD.channel_id)
BEGIN SELECT RAISE(ABORT,'codex_operation_pending'); END;

CREATE TRIGGER codex_quota_window_periods_timestamp BEFORE UPDATE ON codex_quota_window_periods
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'codex_quota_window_periods_timestamp');
END;

CREATE TRIGGER codex_token_revision AFTER UPDATE OF access_token ON codex_oauth_credentials
WHEN OLD.access_token IS NOT NEW.access_token AND NEW.deleted_at IS NULL
BEGIN
    UPDATE upstream_credentials SET revision=ag_md5_uuid(hex(randomblob(32))),updated_at=ag_now()
    WHERE id=NEW.channel_id;
END;

CREATE TRIGGER config_templates_timestamp BEFORE UPDATE ON config_templates
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'config_templates_timestamp');
END;

CREATE TRIGGER group_identity_registry_immutable_delete BEFORE DELETE ON group_identity_registry BEGIN
    SELECT RAISE(ABORT, 'group_identity_registry_immutable');
END;

CREATE TRIGGER group_identity_registry_immutable_update BEFORE UPDATE ON group_identity_registry BEGIN
    SELECT RAISE(ABORT, 'group_identity_registry_immutable');
END;

CREATE TRIGGER model_operation_rules_timestamp BEFORE UPDATE ON model_operation_rules
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'model_operation_rules_timestamp');
END;

CREATE TRIGGER model_routing_profiles_timestamp BEFORE UPDATE ON model_routing_profiles
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'model_routing_profiles_timestamp');
END;

CREATE TRIGGER model_rule_identity_registry_immutable_delete BEFORE DELETE ON model_rule_identity_registry BEGIN
    SELECT RAISE(ABORT, 'model_rule_identity_registry_immutable');
END;

CREATE TRIGGER model_rule_identity_registry_immutable_update BEFORE UPDATE ON model_rule_identity_registry BEGIN
    SELECT RAISE(ABORT, 'model_rule_identity_registry_immutable');
END;

CREATE TRIGGER models_no_hard_delete BEFORE DELETE ON models BEGIN
    SELECT RAISE(ABORT, 'models_no_hard_delete');
END;

CREATE TRIGGER models_routed_identity BEFORE UPDATE OF source_model_id ON models
WHEN NEW.source_model_id IS NOT OLD.source_model_id
 AND EXISTS (SELECT 1 FROM model_routing_profiles WHERE model_id=OLD.id) BEGIN
    SELECT RAISE(ABORT, 'models_routed_identity');
END;

CREATE TRIGGER models_timestamp BEFORE UPDATE ON models
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'models_timestamp');
END;

CREATE TRIGGER models_tombstone BEFORE UPDATE ON models
WHEN OLD.deleted_at IS NOT NULL OR (
    NEW.deleted_at IS NOT NULL AND (
        EXISTS (SELECT 1 FROM channel_capabilities WHERE test_pricing_model_id=NEW.id AND deleted_at IS NULL)
        OR EXISTS (SELECT 1 FROM model_routing_profiles p JOIN model_operation_rules r
                   ON r.model_routing_profile_id=p.id WHERE p.model_id=NEW.id AND r.enabled)
    )
) BEGIN
    SELECT RAISE(ABORT,'models_tombstone');
END;

CREATE TRIGGER operation_rules_active_model_insert BEFORE INSERT ON model_operation_rules
WHEN EXISTS (SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
             WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'operation_rules_active_model');
END;

CREATE TRIGGER operation_rules_active_model_update BEFORE UPDATE ON model_operation_rules
WHEN EXISTS (SELECT 1 FROM model_routing_profiles p JOIN models m ON m.id=p.model_id
             WHERE p.id=NEW.model_routing_profile_id AND m.deleted_at IS NOT NULL)
BEGIN
    SELECT RAISE(ABORT,'operation_rules_active_model');
END;

CREATE TRIGGER profiles_active_pricing_insert BEFORE INSERT ON model_routing_profiles
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'profiles_active_pricing_insert');
END;

CREATE TRIGGER profiles_active_pricing_update BEFORE UPDATE OF model_id ON model_routing_profiles
WHEN EXISTS (SELECT 1 FROM models WHERE id=NEW.model_id AND deleted_at IS NOT NULL) BEGIN
    SELECT RAISE(ABORT, 'profiles_active_pricing_update');
END;

CREATE TRIGGER proxies_timestamp BEFORE UPDATE ON proxies
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'proxies_timestamp');
END;

CREATE TRIGGER registration_invitation_codes_timestamp BEFORE UPDATE ON registration_invitation_codes
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'registration_invitation_codes_timestamp');
END;

CREATE TRIGGER request_log_ingest_ack_guard BEFORE DELETE ON request_log_ingest
WHEN OLD.metered_at IS NULL
  OR NOT EXISTS (SELECT 1 FROM request_metering_facts WHERE id=OLD.request_log_id)
  OR NOT EXISTS (SELECT 1 FROM request_logs WHERE id=OLD.request_log_id) BEGIN
    SELECT RAISE(ABORT, 'request_log_ingest_ack_guard');
END;

CREATE TRIGGER request_logs_immutable_update BEFORE UPDATE ON request_logs BEGIN
    SELECT RAISE(ABORT, 'request_logs_immutable_update');
END;

CREATE TRIGGER request_metering_facts_immutable_delete BEFORE DELETE ON request_metering_facts BEGIN
    SELECT RAISE(ABORT, 'request_metering_facts_immutable_delete');
END;

CREATE TRIGGER request_metering_facts_immutable_update BEFORE UPDATE ON request_metering_facts BEGIN
    SELECT RAISE(ABORT, 'request_metering_facts_immutable_update');
END;

CREATE TRIGGER request_settlement_pending_delete BEFORE DELETE ON request_settlement_pending
WHEN NOT EXISTS (SELECT 1 FROM request_settlements WHERE request_id=OLD.request_id) BEGIN
    SELECT RAISE(ABORT, 'request_settlement_pending_protect');
END;

CREATE TRIGGER request_settlement_pending_update BEFORE UPDATE ON request_settlement_pending BEGIN
    SELECT RAISE(ABORT, 'request_settlement_pending_protect');
END;

CREATE TRIGGER request_settlements_immutable_delete BEFORE DELETE ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_immutable_delete');
END;

CREATE TRIGGER request_settlements_immutable_update BEFORE UPDATE ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_immutable_update');
END;

CREATE TRIGGER request_settlements_validate BEFORE INSERT ON request_settlements BEGIN
    SELECT RAISE(ABORT, 'request_settlements_eligibility_check')
    WHERE NOT EXISTS (
        SELECT 1 FROM request_metering_facts f JOIN api_keys k ON k.id=f.api_key_id AND k.user_id=f.user_id
        WHERE f.id=NEW.request_id AND f.amount_state IN ('priced','zero_by_policy')
          AND f.cost_amount IS NEW.cost_amount AND f.currency IS NEW.currency
    );
END;

CREATE TRIGGER routing_groups_preserve_tombstones_delete BEFORE DELETE ON routing_groups BEGIN
    SELECT RAISE(ABORT, 'routing_groups_preserve_tombstones');
END;

CREATE TRIGGER routing_groups_preserve_tombstones_update BEFORE UPDATE ON routing_groups
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'routing_groups_preserve_tombstones');
END;

CREATE TRIGGER routing_groups_timestamp BEFORE UPDATE ON routing_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'routing_groups_timestamp');
END;

CREATE TRIGGER system_settings_timestamp BEFORE UPDATE ON system_settings
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'system_settings_timestamp');
END;

CREATE TRIGGER upstream_accesses_preserve_tombstones_delete BEFORE DELETE ON upstream_accesses BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_preserve_tombstones');
END;

CREATE TRIGGER upstream_accesses_preserve_tombstones_update BEFORE UPDATE ON upstream_accesses
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_preserve_tombstones');
END;

CREATE TRIGGER upstream_accesses_timestamp BEFORE UPDATE ON upstream_accesses
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'upstream_accesses_timestamp');
END;

CREATE TRIGGER upstream_channels_preserve_tombstones_delete BEFORE DELETE ON upstream_channels BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_preserve_tombstones');
END;

CREATE TRIGGER upstream_channels_preserve_tombstones_update BEFORE UPDATE ON upstream_channels
WHEN OLD.deleted_at IS NOT NULL BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_preserve_tombstones');
END;

CREATE TRIGGER upstream_channels_timestamp BEFORE UPDATE ON upstream_channels
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'upstream_channels_timestamp');
END;

CREATE TRIGGER upstream_credentials_connector_guard BEFORE UPDATE ON upstream_credentials
WHEN NEW.connector_kind<>OLD.connector_kind
BEGIN SELECT RAISE(ABORT,'credential connector is immutable'); END;

CREATE TRIGGER upstream_credentials_lifecycle BEFORE UPDATE ON upstream_credentials
WHEN OLD.deleted_at IS NOT NULL OR NEW.id<>OLD.id OR NEW.kind<>OLD.kind
 OR (NEW.deleted_at IS NOT NULL AND EXISTS (
    SELECT 1 FROM upstream_channels WHERE credential_id=NEW.id AND deleted_at IS NULL
))
BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;

CREATE TRIGGER upstream_credentials_no_delete BEFORE DELETE ON upstream_credentials
BEGIN SELECT RAISE(ABORT,'upstream_credentials_lifecycle'); END;

CREATE TRIGGER user_groups_no_hard_delete BEFORE DELETE ON user_groups BEGIN
    SELECT RAISE(ABORT, 'user_groups_no_hard_delete');
END;

CREATE TRIGGER user_groups_timestamp BEFORE UPDATE ON user_groups
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'user_groups_timestamp');
END;

CREATE TRIGGER users_no_hard_delete BEFORE DELETE ON users BEGIN
    SELECT RAISE(ABORT, 'users_no_hard_delete');
END;

CREATE TRIGGER users_timestamp BEFORE UPDATE ON users
WHEN NEW.updated_at IS NOT ag_now() BEGIN
    SELECT RAISE(ABORT, 'users_timestamp');
END;

CREATE INDEX request_metering_facts_upstream_credential_id_idx ON request_metering_facts (upstream_credential_id) WHERE upstream_credential_id IS NOT NULL;

INSERT INTO user_groups (id, name, description, system_role) VALUES
    ('00000000-0000-0000-0000-000000000101', 'Default Users', 'Default group for newly invited users.', 'user'),
    ('00000000-0000-0000-0000-000000000102', 'Default Administrators', 'Default group for newly invited administrators.', 'admin');
