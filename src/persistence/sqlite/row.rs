//! SQLite `FromRow` mappings for backend-neutral runtime records.

use rust_decimal::Decimal;
use serde_json::Value;
use sqlx::{FromRow, Row, sqlite::SqliteRow, types::Json};
use uuid::Uuid;

use super::super::records::{
    ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, McpServerRecord,
    ModelRecord, ModelRuleRecord, ProxyRecord, SystemSettingsRecord,
};
use super::{SqliteDecimal, SqliteStringList, SqliteUuidList};

fn decimal(row: &SqliteRow, column: &str) -> Result<Decimal, sqlx::Error> {
    row.try_get::<SqliteDecimal, _>(column)
        .map(SqliteDecimal::into_inner)
}

fn optional_decimal(row: &SqliteRow, column: &str) -> Result<Option<Decimal>, sqlx::Error> {
    row.try_get::<Option<SqliteDecimal>, _>(column)
        .map(|value| value.map(SqliteDecimal::into_inner))
}

fn json_value(row: &SqliteRow, column: &str) -> Result<Value, sqlx::Error> {
    row.try_get::<Json<Value>, _>(column).map(|value| value.0)
}

fn string_list(row: &SqliteRow, column: &str) -> Result<Vec<String>, sqlx::Error> {
    row.try_get::<SqliteStringList, _>(column)
        .map(|value| value.0)
}

fn uuid_list(row: &SqliteRow, column: &str) -> Result<Vec<Uuid>, sqlx::Error> {
    row.try_get::<SqliteUuidList, _>(column)
        .map(|value| value.0)
}

impl<'row> FromRow<'row, SqliteRow> for SystemSettingsRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            setting_key: row.try_get("setting_key")?,
            value: json_value(row, "value")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ApiKeyRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            user_id: row.try_get("user_id")?,
            user_status: row.try_get("user_status")?,
            user_websocket_enabled: row.try_get("user_websocket_enabled")?,
            user_filter_fast_mode: row.try_get("user_filter_fast_mode")?,
            secret_value: row.try_get("secret_value")?,
            status: row.try_get("status")?,
            expires_at: row.try_get("expires_at")?,
            allowed_api_formats: string_list(row, "allowed_api_formats")?,
            permissions: string_list(row, "permissions")?,
            allowed_group_ids: uuid_list(row, "allowed_group_ids")?,
            allowed_channel_ids: uuid_list(row, "allowed_channel_ids")?,
            requests_per_minute: row.try_get("requests_per_minute")?,
            max_concurrent_requests: row.try_get("max_concurrent_requests")?,
            quota_limit_amount: optional_decimal(row, "quota_limit_amount")?,
            quota_used_amount: decimal(row, "quota_used_amount")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ModelRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            source_model_id: row.try_get("source_model_id")?,
            currency: row.try_get("currency")?,
            price_unit_tokens: row.try_get("price_unit_tokens")?,
            price_effective_at: row.try_get("price_effective_at")?,
            input_unit_price: decimal(row, "input_unit_price")?,
            cached_input_unit_price: decimal(row, "cached_input_unit_price")?,
            cache_write_unit_price: decimal(row, "cache_write_unit_price")?,
            output_unit_price: decimal(row, "output_unit_price")?,
            advanced_billing: json_value(row, "advanced_billing")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ModelRuleRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            client_model: row.try_get("client_model")?,
            api_format: row.try_get("api_format")?,
            upstream_model_id: row.try_get("upstream_model_id")?,
            upstream_model_enabled: row.try_get("upstream_model_enabled")?,
            upstream_model_currency: row.try_get("upstream_model_currency")?,
            price_unit_tokens: row.try_get("price_unit_tokens")?,
            price_effective_at: row.try_get("price_effective_at")?,
            input_unit_price: decimal(row, "input_unit_price")?,
            cached_input_unit_price: decimal(row, "cached_input_unit_price")?,
            cache_write_unit_price: decimal(row, "cache_write_unit_price")?,
            output_unit_price: decimal(row, "output_unit_price")?,
            advanced_billing: json_value(row, "advanced_billing")?,
            upstream_model: row.try_get("upstream_model")?,
            channel_group_ids: uuid_list(row, "channel_group_ids")?,
            channel_ids: uuid_list(row, "channel_ids")?,
            enabled: row.try_get("enabled")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ChannelGroupRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            api_format: row.try_get("api_format")?,
            connector_kind: row.try_get("connector_kind")?,
            request_compression: row.try_get("request_compression")?,
            priority: row.try_get("priority")?,
            selection_strategy: row.try_get("selection_strategy")?,
            enabled: row.try_get("enabled")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ChannelRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            channel_group_id: row.try_get("channel_group_id")?,
            api_format: row.try_get("api_format")?,
            name: row.try_get("name")?,
            base_url: row.try_get("base_url")?,
            enabled: row.try_get("enabled")?,
            supports_websocket: row.try_get("supports_websocket")?,
            supports_standalone_web_search: row.try_get("supports_standalone_web_search")?,
            auto_disabled: row.try_get("auto_disabled")?,
            auto_disable_allowed: row.try_get("auto_disable_allowed")?,
            weight: row.try_get("weight")?,
            billing_multiplier: decimal(row, "billing_multiplier")?,
            proxy_id: row.try_get("proxy_id")?,
            config_template_id: row.try_get("config_template_id")?,
            override_document: json_value(row, "override_document")?,
            connect_timeout_ms: row.try_get("connect_timeout_ms")?,
            response_header_timeout_ms: row.try_get("response_header_timeout_ms")?,
            stream_idle_timeout_ms: row.try_get("stream_idle_timeout_ms")?,
            upstream_auth_kind: row.try_get("upstream_auth_kind")?,
            upstream_auth_header_name: row.try_get("upstream_auth_header_name")?,
            upstream_api_key: row.try_get("upstream_api_key")?,
            available_models: string_list(row, "available_models")?,
            test_model: row.try_get("test_model")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ProxyRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            proxy_url: row.try_get("proxy_url")?,
            username: row.try_get("username")?,
            password: row.try_get("password")?,
            no_proxy_hosts: string_list(row, "no_proxy_hosts")?,
            enabled: row.try_get("enabled")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for ConfigTemplateRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            document: json_value(row, "document")?,
            enabled: row.try_get("enabled")?,
        })
    }
}

impl<'row> FromRow<'row, SqliteRow> for McpServerRecord {
    fn from_row(row: &'row SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            slug: row.try_get("slug")?,
            kind: row.try_get("kind")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            model_rule_id: row.try_get("model_rule_id")?,
            settings_version: row.try_get("settings_version")?,
            settings: json_value(row, "settings")?,
            enabled: row.try_get("enabled")?,
        })
    }
}
