//! PostgreSQL `FromRow` mappings for backend-neutral persistence records.

use sqlx::{FromRow, Row, postgres::PgRow};

use super::super::{
    auth::{
        ConsoleProfile, LiveConsoleIdentity, LoginUser, PasswordUser, RegistrationInvitationCode,
    },
    control_plane::{
        ConsoleApiKey, ConsoleAuditLog, ControlPlaneApiKey, ControlPlaneApiKeyPolicy,
        ControlPlaneChannelDetail, ControlPlaneChannelGroup, ControlPlaneConfigTemplate,
        ControlPlaneConfigTemplateDetail, ControlPlaneMcpServer, ControlPlaneModel,
        ControlPlaneProxy, ControlPlaneUser, ControlPlaneUserGroup, SelfApiKeyChannelOption,
        SelfApiKeyGroupOption,
    },
    records::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, McpServerRecord,
        ModelRecord, ModelRuleRecord, ProxyRecord, SystemSettingsRecord, UserSettingsView,
    },
    request_log::{
        ConsoleRequestLog, RequestLogIngestBacklog, RequestLogIngestRecord,
        RequestLogSettlementBacklog,
    },
};

macro_rules! impl_postgres_from_row {
    ($record:ty { $($field:ident),+ $(,)? }) => {
        impl<'row> FromRow<'row, PgRow> for $record {
            fn from_row(row: &'row PgRow) -> Result<Self, sqlx::Error> {
                Ok(Self {
                    $($field: row.try_get(stringify!($field))?),+
                })
            }
        }
    };
}

impl_postgres_from_row!(SystemSettingsRecord {
    setting_key,
    value,
    updated_at,
});

impl_postgres_from_row!(UserSettingsView {
    websocket_enabled,
    updated_at,
});

impl_postgres_from_row!(ApiKeyRecord {
    id,
    user_id,
    user_status,
    user_websocket_enabled,
    user_filter_fast_mode,
    secret_value,
    status,
    expires_at,
    allowed_api_formats,
    permissions,
    allowed_group_ids,
    allowed_channel_ids,
    requests_per_minute,
    max_concurrent_requests,
    quota_limit_amount,
    quota_used_amount,
});

impl_postgres_from_row!(ModelRecord {
    id,
    source_model_id,
    currency,
    price_unit_tokens,
    price_effective_at,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    advanced_billing,
});

impl_postgres_from_row!(ModelRuleRecord {
    id,
    client_model,
    api_format,
    upstream_model_id,
    upstream_model_enabled,
    upstream_model_currency,
    price_unit_tokens,
    price_effective_at,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    advanced_billing,
    upstream_model,
    channel_group_ids,
    channel_ids,
    enabled,
});

impl_postgres_from_row!(ChannelGroupRecord {
    id,
    name,
    api_format,
    connector_kind,
    request_compression,
    priority,
    selection_strategy,
    enabled,
});

impl_postgres_from_row!(ChannelRecord {
    id,
    channel_group_id,
    api_format,
    name,
    base_url,
    enabled,
    supports_websocket,
    supports_standalone_web_search,
    auto_disabled,
    auto_disable_allowed,
    weight,
    billing_multiplier,
    proxy_id,
    config_template_id,
    override_document,
    connect_timeout_ms,
    response_header_timeout_ms,
    stream_idle_timeout_ms,
    upstream_auth_kind,
    upstream_auth_header_name,
    upstream_api_key,
    available_models,
    test_model,
});

impl_postgres_from_row!(ProxyRecord {
    id,
    name,
    proxy_url,
    username,
    password,
    no_proxy_hosts,
    enabled,
});

impl_postgres_from_row!(ConfigTemplateRecord {
    id,
    name,
    description,
    document,
    enabled,
});

impl_postgres_from_row!(McpServerRecord {
    id,
    slug,
    kind,
    name,
    description,
    model_rule_id,
    settings_version,
    settings,
    enabled,
});

impl_postgres_from_row!(LoginUser {
    id,
    email,
    display_name,
    role,
    status,
    password_hash,
    auth_version,
    password_change_required,
    temporary_password_expires_at,
});

impl_postgres_from_row!(LiveConsoleIdentity {
    user_id,
    email,
    display_name,
    role,
    status,
    auth_version,
    session_id,
    expires_at,
    revoked_at,
    session_purpose,
});

impl_postgres_from_row!(PasswordUser {
    id,
    password_hash,
    status,
    role,
    auth_version,
    password_change_required,
    temporary_password_expires_at,
});

impl_postgres_from_row!(ConsoleProfile {
    id,
    email,
    display_name,
    role,
    status,
    balance_amount,
    created_at,
    updated_at,
});

impl_postgres_from_row!(RegistrationInvitationCode {
    id,
    name,
    max_uses,
    used_count,
    expires_at,
    enabled,
    user_group_id,
    initial_balance_amount,
    created_by,
    last_used_at,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneUser {
    id,
    email,
    display_name,
    role,
    status,
    can_reissue_invitation,
    password_change_required,
    temporary_password_expires_at,
    user_group_id,
    default_api_key_policy_id,
    effective_api_key_policy_id,
    websocket_enabled,
    balance_amount,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneUserGroup {
    id,
    name,
    description,
    default_api_key_policy_id,
    visible_codex_quota_group_ids,
    filter_fast_mode,
    system_role,
    member_count,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneModel {
    id,
    source_model_id,
    display_name,
    provider_name,
    enabled,
    price_unit_tokens,
    input_unit_price,
    cached_input_unit_price,
    cache_write_unit_price,
    output_unit_price,
    price_effective_at,
    advanced_billing,
    last_synced_at,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneApiKeyPolicy {
    id,
    name,
    allowed_group_ids,
    allowed_channel_ids,
    enabled,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ConsoleApiKey {
    id,
    name,
    secret,
    status,
    expires_at,
    allowed_api_formats,
    permissions,
    allowed_group_ids,
    allowed_channel_ids,
    requests_per_minute,
    max_concurrent_requests,
    quota_limit_amount,
    quota_used_amount,
    created_at,
    updated_at,
});

impl_postgres_from_row!(SelfApiKeyGroupOption {
    id,
    name,
    api_format,
    priority,
    enabled,
});

impl_postgres_from_row!(SelfApiKeyChannelOption {
    id,
    channel_group_id,
    channel_group_name,
    channel_group_enabled,
    api_format,
    name,
    enabled,
    auto_disabled,
});

impl_postgres_from_row!(ConsoleAuditLog {
    id,
    occurred_at,
    actor_user_id,
    actor_type,
    actor_role,
    action,
    object_type,
    object_id,
    before_redacted,
    after_redacted,
    correlation_id,
    reason,
});

impl_postgres_from_row!(ControlPlaneApiKey {
    id,
    user_id,
    user_status,
    name,
    secret,
    status,
    expires_at,
    allowed_api_formats,
    permissions,
    allowed_group_ids,
    allowed_channel_ids,
    requests_per_minute,
    max_concurrent_requests,
    quota_limit_amount,
    quota_used_amount,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneChannelGroup {
    id,
    name,
    api_format,
    connector_kind,
    connector_pool_id,
    request_compression,
    priority,
    selection_strategy,
    enabled,
    status_statistics_enabled,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneChannelDetail {
    id,
    channel_group_id,
    api_format,
    connector_kind,
    provider_managed,
    name,
    base_url,
    enabled,
    supports_websocket,
    supports_standalone_web_search,
    auto_disabled,
    auto_disabled_reason,
    auto_disable_allowed,
    weight,
    billing_multiplier,
    proxy_id,
    config_template_id,
    override_document,
    connect_timeout_ms,
    response_header_timeout_ms,
    stream_idle_timeout_ms,
    upstream_auth_kind,
    upstream_auth_header_name,
    upstream_api_key,
    upstream_credential_configured,
    available_models,
    test_model,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneProxy {
    id,
    name,
    proxy_url,
    no_proxy_hosts,
    enabled,
    credential_configured,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneConfigTemplate {
    id,
    name,
    description,
    api_format,
    enabled,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneConfigTemplateDetail {
    id,
    name,
    description,
    api_format,
    document,
    enabled,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ControlPlaneMcpServer {
    id,
    slug,
    kind,
    name,
    description,
    model_rule_id,
    client_model,
    api_format,
    settings_version,
    settings,
    enabled,
    created_at,
    updated_at,
});

impl_postgres_from_row!(ConsoleRequestLog {
    id,
    started_at,
    completed_at,
    user_id,
    user_name,
    api_key_id,
    request_source,
    api_format,
    api_operation,
    request_protocol,
    client_model,
    reasoning_effort,
    fast_mode,
    upstream_model,
    model_rule_id,
    channel_group_id,
    channel_group_name,
    channel_id,
    channel_name,
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
    cost_amount,
    error_code,
    error_summary,
    billed_at,
});

impl_postgres_from_row!(RequestLogIngestRecord {
    sequence,
    request_log_id,
    schema_version,
    payload,
    attempt_count,
});

impl_postgres_from_row!(RequestLogIngestBacklog {
    row_count,
    oldest_staged_at,
});

impl_postgres_from_row!(RequestLogSettlementBacklog {
    row_count,
    oldest_completed_at,
});
