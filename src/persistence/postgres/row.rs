//! PostgreSQL `FromRow` mappings for backend-neutral runtime records.

use sqlx::{FromRow, Row, postgres::PgRow};

use super::super::records::{
    ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, McpServerRecord,
    ModelRecord, ModelRuleRecord, ProxyRecord, SystemSettingsRecord, UserSettingsView,
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
