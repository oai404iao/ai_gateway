//! SQLx control-plane and append-only request-log repositories.

use std::fmt;

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::RequestLogEvent;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Default)]
pub struct ControlPlaneRecords {
    pub api_keys: Vec<ApiKeyRecord>,
    pub model_rules: Vec<ModelRuleRecord>,
    pub groups: Vec<ChannelGroupRecord>,
    pub channels: Vec<ChannelRecord>,
    pub proxies: Vec<ProxyRecord>,
    pub templates: Vec<ConfigTemplateRecord>,
}

#[derive(FromRow)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub secret_value: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub requests_per_minute: Option<i32>,
    pub tokens_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
}
impl fmt::Debug for ApiKeyRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiKeyRecord")
            .field("id", &self.id)
            .field("user_id", &self.user_id)
            .field("user_status", &self.user_status)
            .field("secret_value", &"REDACTED")
            .field("status", &self.status)
            .field("expires_at", &self.expires_at)
            .field("allowed_api_formats", &self.allowed_api_formats)
            .field("permissions", &self.permissions)
            .field("allowed_group_ids", &self.allowed_group_ids)
            .field("requests_per_minute", &self.requests_per_minute)
            .field("tokens_per_minute", &self.tokens_per_minute)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("quota_limit_amount", &self.quota_limit_amount)
            .field("quota_used_amount", &self.quota_used_amount)
            .finish()
    }
}
#[derive(Debug, FromRow)]
pub struct ModelRuleRecord {
    pub id: Uuid,
    pub client_model: String,
    pub api_format: String,
    pub model_id: Uuid,
    pub model_enabled: bool,
    pub model_currency: String,
    pub price_unit_tokens: i64,
    pub price_effective_at: DateTime<Utc>,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub upstream_model: String,
    pub channel_group_ids: Vec<Uuid>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
}
#[derive(Debug, FromRow)]
pub struct ChannelGroupRecord {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
}
#[derive(Clone, FromRow)]
pub struct ChannelRecord {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub auto_disabled: bool,
    pub weight: i32,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub override_document: Value,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_api_key: Option<String>,
    pub available_models: Vec<String>,
    pub health_check: Value,
}
impl fmt::Debug for ChannelRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChannelRecord")
            .field("id", &self.id)
            .field("channel_group_id", &self.channel_group_id)
            .field("api_format", &self.api_format)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("enabled", &self.enabled)
            .field("auto_disabled", &self.auto_disabled)
            .field("weight", &self.weight)
            .field("proxy_id", &self.proxy_id)
            .field("config_template_id", &self.config_template_id)
            .field("override_document", &"REDACTED")
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field(
                "response_header_timeout_ms",
                &self.response_header_timeout_ms,
            )
            .field("stream_idle_timeout_ms", &self.stream_idle_timeout_ms)
            .field("upstream_auth_kind", &self.upstream_auth_kind)
            .field("upstream_auth_header_name", &self.upstream_auth_header_name)
            .field("upstream_api_key", &"REDACTED")
            .field("available_models", &self.available_models)
            .field("health_check", &self.health_check)
            .finish()
    }
}

#[derive(FromRow)]
pub struct ProxyRecord {
    pub id: Uuid,
    pub name: String,
    pub proxy_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}
impl fmt::Debug for ProxyRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("proxy_url", &"REDACTED")
            .field("username", &"REDACTED")
            .field("password", &"REDACTED")
            .field("no_proxy_hosts", &self.no_proxy_hosts)
            .field("enabled", &self.enabled)
            .finish()
    }
}

#[derive(FromRow)]
pub struct ConfigTemplateRecord {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub document: Value,
    pub enabled: bool,
}
impl fmt::Debug for ConfigTemplateRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfigTemplateRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field("document", &"REDACTED")
            .field("enabled", &self.enabled)
            .finish()
    }
}

#[derive(Clone)]
pub struct ControlPlaneRepository {
    pool: PgPool,
}

/// Explicit, typed management inputs. HTTP owns request decoding; this module
/// owns only fixed SQL statements for these supported resources.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyCreate {
    pub user_id: Uuid,
    pub name: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyUpdate {
    pub name: String,
    pub status: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserInput {
    pub name: String,
    pub status: String,
    pub currency: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInput {
    pub source_model_id: String,
    pub display_name: String,
    #[serde(default)]
    pub provider_name: Option<String>,
    pub enabled: bool,
    pub currency: String,
    pub price_unit_tokens: i64,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub price_effective_at: DateTime<Utc>,
    /// Create defaults to `{}`; omission during an update preserves the
    /// opaque source document which ordinary reads deliberately do not expose.
    #[serde(default)]
    pub source_payload: Option<Value>,
}
/// A fully validated, price-bearing model selected for an explicit catalog
/// import. Unlike `ModelInput`, this is not decoded from an HTTP request.
#[derive(Clone)]
pub struct SyncedModelInput {
    pub source_model_id: String,
    pub display_name: String,
    pub provider_name: String,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub source_payload: Value,
}
/// Identifies a local model that was previously imported from models.dev and
/// is therefore eligible for automatic price refresh.
#[derive(Clone, Debug, FromRow)]
pub struct ModelsDevPriceTarget {
    pub model_id: Uuid,
    pub source_model_id: String,
    pub provider_id: String,
}
/// The new price facts for one existing models.dev-imported local model.
#[derive(Clone)]
pub struct SyncedModelPrice {
    pub model_id: Uuid,
    pub source_model_id: String,
    pub provider_id: String,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub source_payload: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelGroupInput {
    pub name: String,
    pub api_format: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreateInput {
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub weight: i32,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    #[serde(default = "empty_object")]
    pub override_document: Value,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    #[serde(default)]
    pub upstream_auth_header_name: Option<String>,
    pub upstream_api_key: Option<String>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default = "empty_object")]
    pub health_check: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelInput {
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub weight: i32,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    #[serde(default = "empty_object")]
    pub override_document: Value,
    #[serde(default)]
    pub connect_timeout_ms: Option<i32>,
    #[serde(default)]
    pub response_header_timeout_ms: Option<i32>,
    #[serde(default)]
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    #[serde(default)]
    pub upstream_auth_header_name: Option<String>,
    /// Absent keeps the current secret; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_credential")]
    pub upstream_api_key: Option<Option<String>>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default = "empty_object")]
    pub health_check: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRuleInput {
    pub client_model: String,
    pub api_format: String,
    pub model_id: Uuid,
    pub upstream_model: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub channel_group_ids: Vec<Uuid>,
    #[serde(default)]
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyCreateInput {
    pub name: String,
    pub proxy_url: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyInput {
    pub name: String,
    pub proxy_url: String,
    /// Absent keeps the current credential component; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_credential")]
    pub username: Option<Option<String>>,
    /// Absent keeps the current credential component; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_credential")]
    pub password: Option<Option<String>>,
    #[serde(default)]
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTemplateInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub document: Value,
    pub enabled: bool,
}
fn empty_object() -> Value {
    json!({})
}
fn deserialize_optional_credential<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}
struct ChannelMutationInput {
    channel_group_id: Uuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    weight: i32,
    proxy_id: Option<Uuid>,
    config_template_id: Option<Uuid>,
    override_document: Value,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    upstream_auth_kind: String,
    upstream_auth_header_name: Option<String>,
    upstream_api_key: Option<Option<String>>,
    available_models: Vec<String>,
    health_check: Value,
}
impl From<ChannelCreateInput> for ChannelMutationInput {
    fn from(value: ChannelCreateInput) -> Self {
        Self {
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            weight: value.weight,
            proxy_id: value.proxy_id,
            config_template_id: value.config_template_id,
            override_document: value.override_document,
            connect_timeout_ms: value.connect_timeout_ms,
            response_header_timeout_ms: value.response_header_timeout_ms,
            stream_idle_timeout_ms: value.stream_idle_timeout_ms,
            upstream_auth_kind: value.upstream_auth_kind,
            upstream_auth_header_name: value.upstream_auth_header_name,
            upstream_api_key: Some(value.upstream_api_key),
            available_models: value.available_models,
            health_check: value.health_check,
        }
    }
}
impl From<ChannelInput> for ChannelMutationInput {
    fn from(value: ChannelInput) -> Self {
        Self {
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            weight: value.weight,
            proxy_id: value.proxy_id,
            config_template_id: value.config_template_id,
            override_document: value.override_document,
            connect_timeout_ms: value.connect_timeout_ms,
            response_header_timeout_ms: value.response_header_timeout_ms,
            stream_idle_timeout_ms: value.stream_idle_timeout_ms,
            upstream_auth_kind: value.upstream_auth_kind,
            upstream_auth_header_name: value.upstream_auth_header_name,
            upstream_api_key: value.upstream_api_key,
            available_models: value.available_models,
            health_check: value.health_check,
        }
    }
}

pub enum AdminMutation {
    CreateUser(UserInput),
    UpdateUser {
        id: Uuid,
        input: UserInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateModel(ModelInput),
    UpdateModel {
        id: Uuid,
        input: ModelInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateApiKey(ApiKeyCreate),
    UpdateApiKey {
        id: Uuid,
        input: ApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    },
    RevokeApiKey {
        id: Uuid,
        reason: String,
    },
    CreateGroup(ChannelGroupInput),
    UpdateGroup {
        id: Uuid,
        input: ChannelGroupInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateChannel(ChannelCreateInput),
    UpdateChannel {
        id: Uuid,
        input: ChannelInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateRule(ModelRuleInput),
    UpdateRule {
        id: Uuid,
        input: ModelRuleInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateProxy(ProxyCreateInput),
    UpdateProxy {
        id: Uuid,
        input: ProxyInput,
        expected_updated_at: DateTime<Utc>,
    },
    CreateConfigTemplate(ConfigTemplateInput),
    UpdateConfigTemplate {
        id: Uuid,
        input: ConfigTemplateInput,
        expected_updated_at: DateTime<Utc>,
    },
}

pub struct MutationResult {
    pub id: Uuid,
    pub object_type: &'static str,
    pub action: &'static str,
    pub before_redacted: Value,
    pub after_redacted: Value,
    pub created_secret: Option<String>,
    pub reason: Option<String>,
    pub updated_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct AdminLists {
    pub users: Vec<AdminUser>,
    pub models: Vec<AdminModel>,
    pub api_keys: Vec<AdminApiKey>,
    pub channel_groups: Vec<AdminChannelGroup>,
    pub channels: Vec<AdminChannel>,
    pub model_rules: Vec<AdminModelRule>,
    pub proxies: Vec<AdminProxy>,
    pub config_templates: Vec<AdminConfigTemplate>,
}
#[derive(Serialize, FromRow)]
pub struct AdminUser {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub balance_amount: rust_decimal::Decimal,
    pub currency: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct AdminModel {
    pub id: Uuid,
    pub source_model_id: String,
    pub display_name: String,
    pub provider_name: Option<String>,
    pub enabled: bool,
    pub currency: String,
    pub price_unit_tokens: i64,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub price_effective_at: DateTime<Utc>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct AdminApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub name: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub requests_per_minute: Option<i32>,
    pub tokens_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct AdminChannelGroup {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct AdminChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub weight: i32,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_credential_configured: bool,
    pub available_models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(FromRow)]
struct AdminChannelRow {
    id: Uuid,
    channel_group_id: Uuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    auto_disabled: bool,
    auto_disabled_reason: Option<String>,
    weight: i32,
    proxy_id: Option<Uuid>,
    config_template_id: Option<Uuid>,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    upstream_auth_kind: String,
    upstream_auth_header_name: Option<String>,
    upstream_credential_configured: bool,
    available_models: Vec<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
impl From<AdminChannelRow> for AdminChannel {
    fn from(value: AdminChannelRow) -> Self {
        Self {
            id: value.id,
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            auto_disabled: value.auto_disabled,
            auto_disabled_reason: value.auto_disabled_reason,
            weight: value.weight,
            proxy_id: value.proxy_id,
            config_template_id: value.config_template_id,
            connect_timeout_ms: value.connect_timeout_ms,
            response_header_timeout_ms: value.response_header_timeout_ms,
            stream_idle_timeout_ms: value.stream_idle_timeout_ms,
            upstream_auth_kind: value.upstream_auth_kind,
            upstream_auth_header_name: value.upstream_auth_header_name,
            upstream_credential_configured: value.upstream_credential_configured,
            available_models: value.available_models,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}
#[derive(Serialize, FromRow)]
pub struct AdminModelRule {
    pub id: Uuid,
    pub client_model: String,
    pub api_format: String,
    pub model_id: Uuid,
    pub model_enabled: bool,
    pub upstream_model: String,
    pub description: Option<String>,
    pub channel_group_ids: Vec<Uuid>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct AdminProxy {
    pub id: Uuid,
    pub name: String,
    pub proxy_url: String,
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
    pub credential_configured: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct AdminConfigTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct RequestLogRepository {
    pool: PgPool,
}

impl RequestLogRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts one terminal event without changing schema-owned defaults.
    ///
    /// A duplicate id is successful only if every field owned by this event is
    /// identical after PostgreSQL's microsecond timestamp normalization.
    pub async fn insert(
        &self,
        event: &RequestLogEvent,
    ) -> Result<RequestLogInsertOutcome, RepositoryError> {
        let status = event
            .response_status_code
            .map(validate_response_status)
            .transpose()?;
        let billing = event.billing.as_ref();
        let usage = billing.and_then(|billing| billing.usage.as_ref());
        let price = billing.map(|billing| &billing.price);
        let started_at = normalize_timestamp(event.started_at);
        let completed_at = normalize_timestamp(event.completed_at);
        let inserted = sqlx::query_scalar::<_, Uuid>("INSERT INTO request_logs (id, started_at, completed_at, user_id, api_key_id, api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, output_tokens_per_second, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, model_id, currency, price_unit_tokens, price_effective_at, input_unit_price, cached_input_unit_price, cache_write_unit_price, output_unit_price, cost_amount, error_code) VALUES ($1, $2, $3, $4, $5, $6::api_format, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31) ON CONFLICT (id) DO NOTHING RETURNING id")
            .bind(event.id)
            .bind(started_at)
            .bind(completed_at)
            .bind(event.user_id)
            .bind(event.api_key_id)
            .bind(match event.api_format {
                crate::domain::ApiFormat::OpenAiChatCompletions => "open_ai_chat_completions",
                crate::domain::ApiFormat::OpenAiResponses => "open_ai_responses",
            })
            .bind(&event.client_model)
            .bind(&event.upstream_model)
            .bind(event.model_rule_id)
            .bind(event.channel_group_id)
            .bind(event.channel_id)
            .bind(event.outcome.as_str())
            .bind(status)
            .bind(event.streamed)
            .bind(event.ttft_ms)
            .bind(event.total_duration_ms)
            .bind(billing.and_then(|billing| billing.output_tokens_per_second))
            .bind(usage.map(|usage| usage.input_tokens))
            .bind(usage.map(|usage| usage.cached_input_tokens))
            .bind(usage.map(|usage| usage.cache_write_tokens))
            .bind(usage.map(|usage| usage.output_tokens))
            .bind(event.model_id)
            .bind(price.map(|price| &price.currency))
            .bind(price.map(|price| price.price_unit_tokens))
            .bind(price.map(|price| price.price_effective_at))
            .bind(price.map(|price| price.input_unit_price))
            .bind(price.map(|price| price.cached_input_unit_price))
            .bind(price.map(|price| price.cache_write_unit_price))
            .bind(price.map(|price| price.output_unit_price))
            .bind(billing.and_then(|billing| billing.cost_amount))
            .bind(event.error_code)
            .fetch_optional(&self.pool)
            .await?;
        if inserted.is_some() {
            return Ok(RequestLogInsertOutcome::Inserted);
        }

        let existing = sqlx::query_as::<_, StoredRequestLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, output_tokens_per_second, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, model_id, currency, price_unit_tokens, price_effective_at, input_unit_price, cached_input_unit_price, cache_write_unit_price, output_unit_price, cost_amount, error_code FROM request_logs WHERE id = $1")
            .bind(event.id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(RepositoryError::DuplicateDisappeared { id: event.id })?;
        if existing.matches(event, started_at, completed_at, status) {
            Ok(RequestLogInsertOutcome::ExactDuplicate)
        } else {
            Err(RepositoryError::DuplicateConflict { id: event.id })
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogInsertOutcome {
    Inserted,
    ExactDuplicate,
}

#[derive(FromRow)]
struct StoredRequestLog {
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    user_id: Uuid,
    api_key_id: Uuid,
    api_format: String,
    client_model: String,
    upstream_model: Option<String>,
    model_rule_id: Option<Uuid>,
    channel_group_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    outcome: String,
    response_status_code: Option<i16>,
    streamed: bool,
    ttft_ms: Option<i32>,
    total_duration_ms: Option<i32>,
    output_tokens_per_second: Option<rust_decimal::Decimal>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    output_tokens: Option<i64>,
    model_id: Option<Uuid>,
    currency: Option<String>,
    price_unit_tokens: Option<i64>,
    price_effective_at: Option<DateTime<Utc>>,
    input_unit_price: Option<rust_decimal::Decimal>,
    cached_input_unit_price: Option<rust_decimal::Decimal>,
    cache_write_unit_price: Option<rust_decimal::Decimal>,
    output_unit_price: Option<rust_decimal::Decimal>,
    cost_amount: Option<rust_decimal::Decimal>,
    error_code: Option<String>,
}

impl StoredRequestLog {
    fn matches(
        &self,
        event: &RequestLogEvent,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        response_status_code: Option<i16>,
    ) -> bool {
        self.started_at == started_at
            && self.completed_at == completed_at
            && self.user_id == event.user_id
            && self.api_key_id == event.api_key_id
            && self.api_format == api_format_name(event)
            && self.client_model == event.client_model
            && self.upstream_model == event.upstream_model
            && self.model_rule_id == event.model_rule_id
            && self.channel_group_id == event.channel_group_id
            && self.channel_id == event.channel_id
            && self.outcome == event.outcome.as_str()
            && self.response_status_code == response_status_code
            && self.streamed == event.streamed
            && self.ttft_ms == event.ttft_ms
            && self.total_duration_ms == Some(event.total_duration_ms)
            && self.output_tokens_per_second
                == event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.output_tokens_per_second)
            && self.input_tokens
                == event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.usage.as_ref().map(|usage| usage.input_tokens))
            && self.cached_input_tokens
                == event.billing.as_ref().and_then(|billing| {
                    billing
                        .usage
                        .as_ref()
                        .map(|usage| usage.cached_input_tokens)
                })
            && self.cache_write_tokens
                == event.billing.as_ref().and_then(|billing| {
                    billing.usage.as_ref().map(|usage| usage.cache_write_tokens)
                })
            && self.output_tokens
                == event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.usage.as_ref().map(|usage| usage.output_tokens))
            && self.model_id == event.model_id
            && self.currency
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.currency.clone())
            && self.price_unit_tokens
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.price_unit_tokens)
            && self.price_effective_at
                == event
                    .billing
                    .as_ref()
                    .map(|billing| normalize_timestamp(billing.price.price_effective_at))
            && self.input_unit_price
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.input_unit_price)
            && self.cached_input_unit_price
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.cached_input_unit_price)
            && self.cache_write_unit_price
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.cache_write_unit_price)
            && self.output_unit_price
                == event
                    .billing
                    .as_ref()
                    .map(|billing| billing.price.output_unit_price)
            && self.cost_amount
                == event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.cost_amount)
            && self.error_code.as_deref() == event.error_code
    }
}

fn api_format_name(event: &RequestLogEvent) -> &'static str {
    match event.api_format {
        crate::domain::ApiFormat::OpenAiChatCompletions => "open_ai_chat_completions",
        crate::domain::ApiFormat::OpenAiResponses => "open_ai_responses",
    }
}

fn validate_response_status(status: u16) -> Result<i16, RepositoryError> {
    if !(100..=599).contains(&status) {
        return Err(RepositoryError::InvalidResponseStatus { status });
    }
    i16::try_from(status).map_err(|_| RepositoryError::InvalidResponseStatus { status })
}

fn normalize_timestamp(value: DateTime<Utc>) -> DateTime<Utc> {
    value
        .with_nanosecond((value.nanosecond() / 1_000) * 1_000)
        .unwrap_or(value)
}
impl ControlPlaneRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn load(&self) -> Result<ControlPlaneRecords, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *transaction)
            .await?;
        let records = Self::load_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(records)
    }
    pub async fn load_transaction(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<ControlPlaneRecords, RepositoryError> {
        let api_keys = sqlx::query_as::<_, ApiKeyRecord>("SELECT k.id, k.user_id, u.status AS user_status, k.secret_value, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.requests_per_minute, k.tokens_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount FROM api_keys k JOIN users u ON u.id = k.user_id ORDER BY k.id").fetch_all(&mut **transaction).await?;
        let model_rules = sqlx::query_as::<_, ModelRuleRecord>("SELECT r.id, r.client_model, r.api_format::text AS api_format, r.model_id, m.enabled AS model_enabled, m.currency AS model_currency, m.price_unit_tokens, m.price_effective_at, m.input_unit_price, m.cached_input_unit_price, m.cache_write_unit_price, m.output_unit_price, r.upstream_model, r.channel_group_ids, r.channel_ids, r.enabled FROM model_rules r JOIN models m ON m.id = r.model_id ORDER BY r.id").fetch_all(&mut **transaction).await?;
        let groups = sqlx::query_as::<_, ChannelGroupRecord>("SELECT id, name, api_format::text AS api_format, priority, selection_strategy, enabled FROM channel_groups ORDER BY id").fetch_all(&mut **transaction).await?;
        let channels = sqlx::query_as::<_, ChannelRecord>("SELECT id, channel_group_id, api_format::text AS api_format, name, base_url, enabled, auto_disabled, weight, proxy_id, config_template_id, override_document, connect_timeout_ms, response_header_timeout_ms, stream_idle_timeout_ms, upstream_auth_kind, upstream_auth_header_name, upstream_api_key, available_models, health_check FROM channels ORDER BY id").fetch_all(&mut **transaction).await?;
        let proxies = sqlx::query_as::<_, ProxyRecord>("SELECT id, name, proxy_url, username, password, no_proxy_hosts, enabled FROM proxies ORDER BY id").fetch_all(&mut **transaction).await?;
        let templates = sqlx::query_as::<_, ConfigTemplateRecord>(
            "SELECT id, name, description, document, enabled FROM config_templates ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        Ok(ControlPlaneRecords {
            api_keys,
            model_rules,
            groups,
            channels,
            proxies,
            templates,
        })
    }

    pub async fn begin_serializable(&self) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE")
            .execute(&mut *transaction)
            .await?;
        Ok(transaction)
    }

    pub async fn active_user_exists(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<bool, RepositoryError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND status = 'active')",
        )
        .bind(id)
        .fetch_one(&mut **transaction)
        .await?)
    }

    pub async fn admin_lists(&self) -> Result<AdminLists, RepositoryError> {
        let users = sqlx::query_as::<_, AdminUser>(
            "SELECT id,name,status,balance_amount,currency,created_at,updated_at FROM users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let models = sqlx::query_as::<_, AdminModel>("SELECT id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,last_synced_at,created_at,updated_at FROM models ORDER BY id").fetch_all(&self.pool).await?;
        let api_keys = sqlx::query_as::<_, AdminApiKey>("SELECT k.id, k.user_id, u.status AS user_status, k.name, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.requests_per_minute, k.tokens_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount, k.updated_at FROM api_keys k JOIN users u ON u.id=k.user_id ORDER BY k.id").fetch_all(&self.pool).await?;
        let channel_groups = sqlx::query_as::<_, AdminChannelGroup>("SELECT id,name,api_format::text AS api_format,priority,selection_strategy,enabled,updated_at FROM channel_groups ORDER BY id").fetch_all(&self.pool).await?;
        let channels = sqlx::query_as::<_, AdminChannelRow>("SELECT id,channel_group_id,api_format::text AS api_format,name,base_url,enabled,auto_disabled,auto_disabled_reason,weight,proxy_id,config_template_id,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,(upstream_api_key IS NOT NULL) AS upstream_credential_configured,available_models,created_at,updated_at FROM channels ORDER BY id").fetch_all(&self.pool).await?;
        let model_rules = sqlx::query_as::<_, AdminModelRule>("SELECT r.id,r.client_model,r.api_format::text AS api_format,r.model_id,m.enabled AS model_enabled,r.upstream_model,r.description,r.channel_group_ids,r.channel_ids,r.enabled,r.updated_at FROM model_rules r JOIN models m ON m.id=r.model_id ORDER BY r.id").fetch_all(&self.pool).await?;
        let proxies = sqlx::query_as::<_, AdminProxy>("SELECT id,name,regexp_replace(regexp_replace(proxy_url, '^([^:/?#]+://)[^/?#]*@', E'\\1'), '[?#].*$', '') AS proxy_url,no_proxy_hosts,enabled,(username IS NOT NULL OR password IS NOT NULL) AS credential_configured,created_at,updated_at FROM proxies ORDER BY id").fetch_all(&self.pool).await?;
        let config_templates = sqlx::query_as::<_, AdminConfigTemplate>("SELECT id,name,description,enabled,created_at,updated_at FROM config_templates ORDER BY id").fetch_all(&self.pool).await?;
        Ok(AdminLists {
            users,
            models,
            api_keys,
            channel_groups,
            channels: channels.into_iter().map(Into::into).collect(),
            model_rules,
            proxies,
            config_templates,
        })
    }

    pub async fn apply_admin_mutation(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        mutation: AdminMutation,
    ) -> Result<MutationResult, RepositoryError> {
        match mutation {
            AdminMutation::CreateUser(input) => {
                user_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateUser {
                id,
                input,
                expected_updated_at,
            } => user_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            AdminMutation::CreateModel(input) => {
                model_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateModel {
                id,
                input,
                expected_updated_at,
            } => model_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            AdminMutation::CreateApiKey(input) => {
                let id = Uuid::new_v4();
                // Two independent UUIDv4 values provide 32 random bytes in a transport-safe form.
                let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                let updated_at = sqlx::query_scalar("INSERT INTO api_keys (id, user_id, name, secret_value, status, expires_at, allowed_api_formats, permissions, allowed_group_ids, requests_per_minute, max_concurrent_requests, quota_limit_amount) VALUES ($1,$2,$3,$4,'active',$5,$6::api_format[],$7,$8,$9,$10,$11) RETURNING updated_at")
                    .bind(id).bind(input.user_id).bind(&input.name).bind(&secret).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).bind(input.requests_per_minute).bind(input.max_concurrent_requests).bind(input.quota_limit_amount).fetch_one(&mut **transaction).await?;
                Ok(MutationResult {
                    id,
                    object_type: "api_key",
                    action: "create",
                    before_redacted: json!({}),
                    after_redacted: key_audit(transaction, id).await?,
                    created_secret: Some(secret),
                    reason: None,
                    updated_at,
                    correlation_id: None,
                })
            }
            AdminMutation::UpdateApiKey {
                id,
                input,
                expected_updated_at,
            } => {
                let before = key_audit(transaction, id).await?;
                let updated_at = sqlx::query_scalar("UPDATE api_keys SET name=$2,status=$3,expires_at=$4,allowed_api_formats=$5::api_format[],permissions=$6,allowed_group_ids=$7,requests_per_minute=$8,max_concurrent_requests=$9,quota_limit_amount=$10 WHERE id=$1 AND updated_at=$11 AND NOT (status='revoked' AND $3 <> 'revoked') RETURNING updated_at")
                    .bind(id).bind(&input.name).bind(&input.status).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).bind(input.requests_per_minute).bind(input.max_concurrent_requests).bind(input.quota_limit_amount).bind(expected_updated_at).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?;
                Ok(MutationResult {
                    id,
                    object_type: "api_key",
                    action: "update",
                    before_redacted: before,
                    after_redacted: key_audit(transaction, id).await?,
                    created_secret: None,
                    reason: None,
                    updated_at,
                    correlation_id: None,
                })
            }
            AdminMutation::RevokeApiKey { id, reason } => {
                if reason.trim().is_empty() {
                    return Err(RepositoryError::Validation);
                }
                let before = key_audit(transaction, id).await?;
                let updated_at = sqlx::query_scalar(
                    "UPDATE api_keys SET status='revoked' WHERE id=$1 AND status <> 'revoked' RETURNING updated_at",
                )
                .bind(id)
                .fetch_optional(&mut **transaction)
                .await?
                .ok_or(RepositoryError::Conflict)?;
                Ok(MutationResult {
                    id,
                    object_type: "api_key",
                    action: "revoke",
                    before_redacted: before,
                    after_redacted: key_audit(transaction, id).await?,
                    created_secret: None,
                    reason: Some(reason),
                    updated_at,
                    correlation_id: None,
                })
            }
            AdminMutation::CreateGroup(input) => {
                group_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateGroup {
                id,
                input,
                expected_updated_at,
            } => group_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            AdminMutation::CreateChannel(input) => {
                channel_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateChannel {
                id,
                input,
                expected_updated_at,
            } => channel_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            AdminMutation::CreateRule(input) => {
                rule_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateRule {
                id,
                input,
                expected_updated_at,
            } => rule_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            AdminMutation::CreateProxy(input) => {
                proxy_insert(transaction, Uuid::new_v4(), input).await
            }
            AdminMutation::UpdateProxy {
                id,
                input,
                expected_updated_at,
            } => proxy_update(transaction, id, input, expected_updated_at).await,
            AdminMutation::CreateConfigTemplate(input) => {
                config_template_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            AdminMutation::UpdateConfigTemplate {
                id,
                input,
                expected_updated_at,
            } => {
                config_template_insert(transaction, id, input, false, Some(expected_updated_at))
                    .await
            }
        }
    }

    pub async fn models_dev_price_targets(
        &self,
    ) -> Result<Vec<ModelsDevPriceTarget>, RepositoryError> {
        sqlx::query_as::<_, ModelsDevPriceTarget>(
            "SELECT id AS model_id,source_model_id,source_payload->>'provider_id' AS provider_id FROM models WHERE source_payload->>'source'='models.dev' AND source_payload ? 'provider_id' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn model_source_ids(&self) -> Result<Vec<String>, RepositoryError> {
        sqlx::query_scalar("SELECT source_model_id FROM models ORDER BY source_model_id")
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::from)
    }

    /// Imports explicitly selected catalog entries. Existing local models are
    /// never overwritten by this path; price refresh is a separate operation.
    pub async fn import_models(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        inputs: Vec<SyncedModelInput>,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        let synced_at = Utc::now();
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(import_model(transaction, input, synced_at).await?);
        }
        Ok(results)
    }

    /// Applies fresh catalog prices only to already imported local models.
    pub async fn sync_model_prices(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        inputs: Vec<SyncedModelPrice>,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        let synced_at = Utc::now();
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(sync_model_price(transaction, input, synced_at).await?);
        }
        Ok(results)
    }

    pub async fn insert_audit(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor: Uuid,
        mutation: &MutationResult,
        correlation_id: Uuid,
    ) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO audit_logs (id,actor_user_id,actor_type,action,object_type,object_id,before_redacted,after_redacted,correlation_id,reason) VALUES ($1,$2,'user',$3,$4,$5,$6,$7,$8,$9)")
            .bind(Uuid::new_v4()).bind(actor).bind(mutation.action).bind(mutation.object_type).bind(mutation.id).bind(&mutation.before_redacted).bind(&mutation.after_redacted).bind(correlation_id.to_string()).bind(&mutation.reason).execute(&mut **transaction).await?;
        Ok(())
    }
    pub async fn insert_manual_reload_audit(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor: Uuid,
        correlation_id: Uuid,
    ) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO audit_logs (id,actor_user_id,actor_type,action,object_type,object_id,before_redacted,after_redacted,correlation_id) VALUES ($1,$2,'user','reload','runtime_config',$3,'{}','{}',$4)")
            .bind(Uuid::new_v4()).bind(actor).bind(Uuid::nil()).bind(correlation_id.to_string()).execute(&mut **transaction).await?;
        Ok(())
    }
}

async fn key_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'user_id',user_id,'name',name,'status',status,'expires_at',expires_at,'allowed_api_formats',allowed_api_formats,'permissions',permissions,'allowed_group_ids',allowed_group_ids,'requests_per_minute',requests_per_minute,'tokens_per_minute',tokens_per_minute,'max_concurrent_requests',max_concurrent_requests,'quota_limit_amount',quota_limit_amount,'quota_used_amount',quota_used_amount,'created_at',created_at,'updated_at',updated_at) FROM api_keys WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn user_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'name',name,'status',status,'balance_amount',balance_amount,'currency',currency,'created_at',created_at,'updated_at',updated_at) FROM users WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    value.ok_or(RepositoryError::NotFound)
}
async fn model_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'source_model_id',source_model_id,'display_name',display_name,'provider_name',provider_name,'enabled',enabled,'currency',currency,'price_unit_tokens',price_unit_tokens,'input_unit_price',input_unit_price,'cached_input_unit_price',cached_input_unit_price,'cache_write_unit_price',cache_write_unit_price,'output_unit_price',output_unit_price,'price_effective_at',price_effective_at,'last_synced_at',last_synced_at,'created_at',created_at,'updated_at',updated_at) FROM models WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    value.ok_or(RepositoryError::NotFound)
}
async fn group_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(channel_groups) FROM channel_groups WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn channel_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    // Keep this allowlist aligned with `AdminChannel`: channel documents are
    // intentionally opaque today and must never leave the database through
    // either management responses or audit snapshots.
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'channel_group_id',channel_group_id,'api_format',api_format,'name',name,'base_url',base_url,'enabled',enabled,'auto_disabled',auto_disabled,'auto_disabled_reason',auto_disabled_reason,'weight',weight,'proxy_id',proxy_id,'config_template_id',config_template_id,'connect_timeout_ms',connect_timeout_ms,'response_header_timeout_ms',response_header_timeout_ms,'stream_idle_timeout_ms',stream_idle_timeout_ms,'upstream_auth_kind',upstream_auth_kind,'upstream_auth_header_name',upstream_auth_header_name,'upstream_credential_configured',(upstream_api_key IS NOT NULL),'available_models',available_models,'created_at',created_at,'updated_at',updated_at) FROM channels WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn rule_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT to_jsonb(model_rules) FROM model_rules WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn proxy_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'name',name,'proxy_url',regexp_replace(regexp_replace(proxy_url, '^([^:/?#]+://)[^/?#]*@', E'\\1'), '[?#].*$', ''),'no_proxy_hosts',no_proxy_hosts,'enabled',enabled,'credential_configured',(username IS NOT NULL OR password IS NOT NULL),'created_at',created_at,'updated_at',updated_at) FROM proxies WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn config_template_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'name',name,'description',description,'enabled',enabled,'created_at',created_at,'updated_at',updated_at) FROM config_templates WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
}
async fn group_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ChannelGroupInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let before = if create {
        json!({})
    } else {
        group_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO channel_groups (id,name,api_format,priority,selection_strategy,enabled) VALUES ($1,$2,$3::api_format,$4,$5,$6) RETURNING updated_at").bind(id).bind(&input.name).bind(&input.api_format).bind(input.priority).bind(&input.selection_strategy).bind(input.enabled).fetch_one(&mut **transaction).await?
    } else {
        sqlx::query_scalar("UPDATE channel_groups SET name=$2,api_format=$3::api_format,priority=$4,selection_strategy=$5,enabled=$6 WHERE id=$1 AND updated_at=$7 RETURNING updated_at").bind(id).bind(&input.name).bind(&input.api_format).bind(input.priority).bind(&input.selection_strategy).bind(input.enabled).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "channel_group",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: group_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn user_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: UserInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let before = if create {
        json!({})
    } else {
        user_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar(
            "INSERT INTO users (id,name,status,currency) VALUES ($1,$2,$3,$4) RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.status)
        .bind(&input.currency)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE users SET name=$2,status=$3,currency=$4 WHERE id=$1 AND updated_at=$5 RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.status)
        .bind(&input.currency)
        .bind(expected_updated_at.expect("PUT version"))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "user",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn model_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ModelInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input
        .source_payload
        .as_ref()
        .is_some_and(|payload| payload.as_object().is_none())
    {
        return Err(RepositoryError::Validation);
    }
    let source_payload_present = input.source_payload.is_some();
    let source_payload = input.source_payload.unwrap_or_else(empty_object);
    let before = if create {
        json!({})
    } else {
        model_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO models (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,source_payload) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) RETURNING updated_at")
            .bind(id)
            .bind(&input.source_model_id)
            .bind(&input.display_name)
            .bind(&input.provider_name)
            .bind(input.enabled)
            .bind(&input.currency)
            .bind(input.price_unit_tokens)
            .bind(input.input_unit_price)
            .bind(input.cached_input_unit_price)
            .bind(input.cache_write_unit_price)
            .bind(input.output_unit_price)
            .bind(input.price_effective_at)
            .bind(&source_payload)
            .fetch_one(&mut **transaction)
            .await?
    } else {
        sqlx::query_scalar("UPDATE models SET source_model_id=$2,display_name=$3,provider_name=$4,enabled=$5,currency=$6,price_unit_tokens=$7,input_unit_price=$8,cached_input_unit_price=$9,cache_write_unit_price=$10,output_unit_price=$11,price_effective_at=$12,source_payload=CASE WHEN $13 THEN $14 ELSE source_payload END WHERE id=$1 AND updated_at=$15 RETURNING updated_at")
            .bind(id)
            .bind(&input.source_model_id)
            .bind(&input.display_name)
            .bind(&input.provider_name)
            .bind(input.enabled)
            .bind(&input.currency)
            .bind(input.price_unit_tokens)
            .bind(input.input_unit_price)
            .bind(input.cached_input_unit_price)
            .bind(input.cache_write_unit_price)
            .bind(input.output_unit_price)
            .bind(input.price_effective_at)
            .bind(source_payload_present)
            .bind(&source_payload)
            .bind(expected_updated_at.expect("PUT version"))
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "model",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn import_model(
    transaction: &mut Transaction<'_, Postgres>,
    input: SyncedModelInput,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let exists =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM models WHERE source_model_id=$1 FOR UPDATE")
            .bind(&input.source_model_id)
            .fetch_optional(&mut **transaction)
            .await?
            .is_some();
    if exists {
        return Err(RepositoryError::Conflict);
    }
    let id = Uuid::new_v4();
    let updated_at = sqlx::query_scalar("INSERT INTO models (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,source_payload,last_synced_at) VALUES ($1,$2,$3,$4,true,'USD',1000000,$5,$6,$7,$8,$9,$10,$11) RETURNING updated_at")
        .bind(id)
        .bind(&input.source_model_id)
        .bind(&input.display_name)
        .bind(&input.provider_name)
        .bind(input.input_unit_price)
        .bind(input.cached_input_unit_price)
        .bind(input.cache_write_unit_price)
        .bind(input.output_unit_price)
        .bind(synced_at)
        .bind(&input.source_payload)
        .bind(synced_at)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(MutationResult {
        id,
        object_type: "model",
        action: "import",
        before_redacted: json!({}),
        after_redacted: model_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn sync_model_price(
    transaction: &mut Transaction<'_, Postgres>,
    input: SyncedModelPrice,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let before = model_audit(transaction, input.model_id).await?;
    let updated_at = sqlx::query_scalar("UPDATE models SET currency='USD',price_unit_tokens=1000000,input_unit_price=$4,cached_input_unit_price=$5,cache_write_unit_price=$6,output_unit_price=$7,price_effective_at=$8,source_payload=$9,last_synced_at=$10 WHERE id=$1 AND source_model_id=$2 AND source_payload->>'source'='models.dev' AND source_payload->>'provider_id'=$3 RETURNING updated_at")
        .bind(input.model_id)
        .bind(&input.source_model_id)
        .bind(&input.provider_id)
        .bind(input.input_unit_price)
        .bind(input.cached_input_unit_price)
        .bind(input.cache_write_unit_price)
        .bind(input.output_unit_price)
        .bind(synced_at)
        .bind(&input.source_payload)
        .bind(synced_at)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id: input.model_id,
        object_type: "model",
        action: "price_sync",
        before_redacted: before,
        after_redacted: model_audit(transaction, input.model_id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn channel_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: impl Into<ChannelMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if input.override_document.as_object().is_none() || !is_empty_document(&input.health_check) {
        return Err(RepositoryError::Validation);
    }
    if matches!(input.upstream_api_key, Some(None)) && input.upstream_auth_kind != "none" {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        channel_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,weight,proxy_id,config_template_id,override_document,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,upstream_api_key,available_models,health_check) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18) RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.proxy_id).bind(input.config_template_id).bind(&input.override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.health_check).fetch_one(&mut **transaction).await?
    } else {
        let credential_present = input.upstream_api_key.is_some();
        sqlx::query_scalar("UPDATE channels SET channel_group_id=$2,api_format=$3::api_format,name=$4,base_url=$5,enabled=$6,weight=$7,proxy_id=$8,config_template_id=$9,override_document=$10,connect_timeout_ms=$11,response_header_timeout_ms=$12,stream_idle_timeout_ms=$13,upstream_auth_kind=$14,upstream_auth_header_name=$15,upstream_api_key=CASE WHEN $16 THEN $17 ELSE upstream_api_key END,available_models=$18,health_check=$19 WHERE id=$1 AND updated_at=$20 RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.proxy_id).bind(input.config_template_id).bind(&input.override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(credential_present).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.health_check).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "channel",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
fn is_empty_document(value: &Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
}
async fn rule_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ModelRuleInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let before = if create {
        json!({})
    } else {
        rule_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO model_rules (id,client_model,api_format,model_id,upstream_model,description,channel_group_ids,channel_ids,enabled) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7,$8,$9) RETURNING updated_at").bind(id).bind(&input.client_model).bind(&input.api_format).bind(input.model_id).bind(&input.upstream_model).bind(&input.description).bind(&input.channel_group_ids).bind(&input.channel_ids).bind(input.enabled).fetch_one(&mut **transaction).await?
    } else {
        sqlx::query_scalar("UPDATE model_rules SET client_model=$2,api_format=$3::api_format,model_id=$4,upstream_model=$5,description=$6,channel_group_ids=$7,channel_ids=$8,enabled=$9 WHERE id=$1 AND updated_at=$10 RETURNING updated_at").bind(id).bind(&input.client_model).bind(&input.api_format).bind(input.model_id).bind(&input.upstream_model).bind(&input.description).bind(&input.channel_group_ids).bind(&input.channel_ids).bind(input.enabled).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "model_rule",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: rule_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn proxy_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ProxyCreateInput,
) -> Result<MutationResult, RepositoryError> {
    let updated_at = sqlx::query_scalar("INSERT INTO proxies (id,name,proxy_url,username,password,no_proxy_hosts,enabled) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING updated_at")
        .bind(id)
        .bind(&input.name)
        .bind(&input.proxy_url)
        .bind(&input.username)
        .bind(&input.password)
        .bind(&input.no_proxy_hosts)
        .bind(input.enabled)
        .fetch_one(&mut **transaction)
        .await?;
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "create",
        before_redacted: json!({}),
        after_redacted: proxy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn proxy_update(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ProxyInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = proxy_audit(transaction, id).await?;
    let username_present = input.username.is_some();
    let password_present = input.password.is_some();
    let updated_at = sqlx::query_scalar("UPDATE proxies SET name=$2,proxy_url=$3,username=CASE WHEN $4 THEN $5 ELSE username END,password=CASE WHEN $6 THEN $7 ELSE password END,no_proxy_hosts=$8,enabled=$9 WHERE id=$1 AND updated_at=$10 RETURNING updated_at")
        .bind(id)
        .bind(&input.name)
        .bind(&input.proxy_url)
        .bind(username_present)
        .bind(input.username.flatten())
        .bind(password_present)
        .bind(input.password.flatten())
        .bind(&input.no_proxy_hosts)
        .bind(input.enabled)
        .bind(expected_updated_at)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "update",
        before_redacted: before,
        after_redacted: proxy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}
async fn config_template_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ConfigTemplateInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let before = if create {
        json!({})
    } else {
        config_template_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO config_templates (id,name,description,document,enabled) VALUES ($1,$2,$3,$4,$5) RETURNING updated_at")
            .bind(id)
            .bind(&input.name)
            .bind(&input.description)
            .bind(&input.document)
            .bind(input.enabled)
            .fetch_one(&mut **transaction)
            .await?
    } else {
        sqlx::query_scalar("UPDATE config_templates SET name=$2,description=$3,document=$4,enabled=$5 WHERE id=$1 AND updated_at=$6 RETURNING updated_at")
            .bind(id)
            .bind(&input.name)
            .bind(&input.description)
            .bind(&input.document)
            .bind(input.enabled)
            .bind(expected_updated_at.expect("PUT version"))
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "config_template",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: config_template_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

#[derive(Debug, Error)]
pub enum RepositoryError {
    #[error("control-plane database operation failed")]
    Sql(#[from] sqlx::Error),
    #[error("request log response status is outside the HTTP range")]
    InvalidResponseStatus { status: u16 },
    #[error("request log id already exists with different immutable facts")]
    DuplicateConflict { id: Uuid },
    #[error("request log duplicate disappeared before it could be compared")]
    DuplicateDisappeared { id: Uuid },
    #[error("requested record was not found or cannot be changed")]
    NotFound,
    #[error("management record version conflicts with the current version")]
    Conflict,
    #[error("management input is invalid")]
    Validation,
}
