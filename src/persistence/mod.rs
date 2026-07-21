//! SQLx control-plane and append-only request-log repositories.

mod auth;

pub use auth::{
    AuthRepository, ConsoleProfile, ConsoleSession, InvitationCreated, InviteUserInput,
    LiveConsoleIdentity, LoginUser, PasswordUser, SessionRotation, SessionUser,
};

use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder, Transaction};
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
    pub upstream_model_id: Uuid,
    pub upstream_model_enabled: bool,
    pub upstream_model_currency: String,
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

/// Administrator-owned template copied into a user's self-service API Key.
/// Users never submit the authorization-bearing fields directly.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyPolicyInput {
    pub name: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    #[serde(default)]
    pub allowed_group_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    pub requests_per_minute: Option<i32>,
    #[serde(default)]
    pub max_concurrent_requests: Option<i32>,
    #[serde(default)]
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub max_active_keys: i32,
    pub enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyCreate {
    pub name: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyUpdate {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserInput {
    #[serde(alias = "name")]
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default = "default_user_role")]
    pub role: String,
    pub status: String,
    pub balance_amount: rust_decimal::Decimal,
    #[serde(default)]
    pub default_api_key_policy_id: Option<Uuid>,
}

fn default_user_role() -> String {
    "user".into()
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelInput {
    pub source_model_id: String,
    pub display_name: String,
    #[serde(default)]
    pub provider_name: Option<String>,
    pub enabled: bool,
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
/// A fully validated, price-bearing models.dev catalog entry selected by an
/// administrator. Unlike `ModelInput`, this is not decoded from an HTTP request.
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
    #[serde(default)]
    pub status_statistics_enabled: bool,
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
    #[serde(default)]
    pub status_statistics_enabled: bool,
    pub weight: i32,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    /// Omission preserves the current opaque transform document; a present
    /// value replaces it (including `{}` to clear it).
    #[serde(default, deserialize_with = "deserialize_optional_document")]
    pub override_document: Option<Value>,
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
    pub upstream_model_id: Uuid,
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
pub struct ConfigTemplateCreateInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub document: Value,
    pub enabled: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigTemplateInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Template documents are deliberately redacted from reads. Omission on
    /// update therefore preserves the stored document; a present value
    /// replaces it.
    #[serde(default, deserialize_with = "deserialize_optional_document")]
    pub document: Option<Value>,
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
fn deserialize_optional_document<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(Some)
}
struct ChannelMutationInput {
    channel_group_id: Uuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    status_statistics_enabled: bool,
    weight: i32,
    proxy_id: Option<Uuid>,
    config_template_id: Option<Uuid>,
    override_document: Option<Value>,
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
            status_statistics_enabled: value.status_statistics_enabled,
            weight: value.weight,
            proxy_id: value.proxy_id,
            config_template_id: value.config_template_id,
            override_document: Some(value.override_document),
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
            status_statistics_enabled: value.status_statistics_enabled,
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
struct ConfigTemplateMutationInput {
    name: String,
    description: Option<String>,
    document: Option<Value>,
    enabled: bool,
}
impl From<ConfigTemplateCreateInput> for ConfigTemplateMutationInput {
    fn from(value: ConfigTemplateCreateInput) -> Self {
        Self {
            name: value.name,
            description: value.description,
            document: Some(value.document),
            enabled: value.enabled,
        }
    }
}
impl From<ConfigTemplateInput> for ConfigTemplateMutationInput {
    fn from(value: ConfigTemplateInput) -> Self {
        Self {
            name: value.name,
            description: value.description,
            document: value.document,
            enabled: value.enabled,
        }
    }
}

pub enum ControlPlaneMutation {
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
    CreateApiKeyPolicy(ApiKeyPolicyInput),
    UpdateApiKeyPolicy {
        id: Uuid,
        input: ApiKeyPolicyInput,
        expected_updated_at: DateTime<Utc>,
    },
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
    CreateConfigTemplate(ConfigTemplateCreateInput),
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
pub struct ControlPlaneLists {
    pub users: Vec<ControlPlaneUser>,
    pub models: Vec<ControlPlaneModel>,
    pub api_keys: Vec<ControlPlaneApiKey>,
    pub api_key_policies: Vec<ControlPlaneApiKeyPolicy>,
    pub channel_groups: Vec<ControlPlaneChannelGroup>,
    pub channels: Vec<ControlPlaneChannel>,
    pub model_rules: Vec<ControlPlaneModelRule>,
    pub proxies: Vec<ControlPlaneProxy>,
    pub config_templates: Vec<ControlPlaneConfigTemplate>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneUser {
    pub id: Uuid,
    pub email: Option<String>,
    pub display_name: String,
    pub role: String,
    pub status: String,
    pub default_api_key_policy_id: Option<Uuid>,
    pub balance_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneModel {
    pub id: Uuid,
    pub source_model_id: String,
    pub display_name: String,
    pub provider_name: Option<String>,
    pub enabled: bool,
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
pub struct ControlPlaneApiKeyPolicy {
    pub id: Uuid,
    pub name: String,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub max_active_keys: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, FromRow)]
pub struct ConsoleApiKey {
    pub id: Uuid,
    pub name: String,
    pub secret: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Option<Vec<Uuid>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ConsoleRequestLog {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub api_key_id: Uuid,
    pub api_format: String,
    pub client_model: String,
    pub upstream_model: Option<String>,
    pub model_rule_id: Option<Uuid>,
    pub channel_group_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub outcome: String,
    pub response_status_code: Option<i16>,
    pub streamed: bool,
    pub ttft_ms: Option<i32>,
    pub total_duration_ms: Option<i32>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_amount: Option<rust_decimal::Decimal>,
    pub error_code: Option<String>,
    pub billed_at: Option<DateTime<Utc>>,
}

/// Server-side request-log filters. The Console API decodes and validates
/// query strings before constructing this typed repository input.
#[derive(Clone, Debug, Default)]
pub struct RequestLogFilter {
    pub limit: i64,
    pub user_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub model: Option<String>,
    pub api_format: Option<String>,
    pub outcome: Option<String>,
    pub started_after: Option<DateTime<Utc>>,
    pub started_before: Option<DateTime<Utc>>,
    pub billed: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelStatusWindow {
    Last24Hours,
    Last3Days,
    Last7Days,
}

impl ChannelStatusWindow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Last24Hours => "24h",
            Self::Last3Days => "3d",
            Self::Last7Days => "7d",
        }
    }

    const fn bucket_seconds(self) -> i64 {
        match self {
            Self::Last24Hours => 30 * 60,
            Self::Last3Days => 2 * 60 * 60,
            Self::Last7Days => 4 * 60 * 60,
        }
    }

    const fn bucket_count(self) -> i64 {
        match self {
            Self::Last24Hours => 48,
            Self::Last3Days => 36,
            Self::Last7Days => 42,
        }
    }

    fn range(self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let bucket_seconds = self.bucket_seconds();
        let current_bucket_started_at = now.timestamp().div_euclid(bucket_seconds) * bucket_seconds;
        let started_at = current_bucket_started_at
            .saturating_sub((self.bucket_count() - 1).saturating_mul(bucket_seconds));
        (DateTime::from_timestamp(started_at, 0).unwrap_or(now), now)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatisticsGranularity {
    Hour,
    Day,
}

impl StatisticsGranularity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    const fn max_range(self) -> chrono::Duration {
        match self {
            Self::Hour => chrono::Duration::days(31),
            Self::Day => chrono::Duration::days(366),
        }
    }

    const fn bucket_seconds(self) -> i64 {
        match self {
            Self::Hour => 60 * 60,
            Self::Day => 24 * 60 * 60,
        }
    }

    const fn bucket_expression(self) -> &'static str {
        match self {
            Self::Hour => "date_trunc('hour', started_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
            Self::Day => "date_trunc('day', started_at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'",
        }
    }
}

#[derive(Clone, Debug)]
pub struct CostStatisticsFilter {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub granularity: StatisticsGranularity,
    pub user_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatusReport {
    pub window: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub bucket_seconds: i64,
    pub models: Vec<ChannelStatusModelMetric>,
    pub channels: Vec<ChannelStatusChannel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatusModelMetric {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatusChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub api_format: String,
    pub name: String,
    pub enabled: bool,
    pub auto_disabled: bool,
    pub models: Vec<ChannelStatusChannelModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatusChannelModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
    pub history: Vec<ChannelStatusBucket>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelStatusBucket {
    pub started_at: DateTime<Utc>,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsReport {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub granularity: String,
    pub summary: CostStatisticsSummary,
    pub buckets: Vec<CostStatisticsBucket>,
    pub models: Vec<CostStatisticsModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsSummary {
    pub request_count: i64,
    pub priced_request_count: i64,
    pub total_tokens: i64,
    pub average_rpm: f64,
    pub average_tpm: f64,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsBucket {
    pub started_at: DateTime<Utc>,
    pub request_count: i64,
    pub total_tokens: i64,
    pub cost_amount: rust_decimal::Decimal,
    pub models: Vec<CostStatisticsBucketModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsBucketModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub success_rate: Option<f64>,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ConsoleAuditLog {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub actor_user_id: Option<Uuid>,
    pub actor_type: String,
    pub actor_role: Option<String>,
    pub action: String,
    pub object_type: String,
    pub object_id: Uuid,
    pub before_redacted: Option<Value>,
    pub after_redacted: Option<Value>,
    pub correlation_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, FromRow)]
pub struct ControlPlaneApiKey {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub name: String,
    pub secret: String,
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
pub struct ControlPlaneChannelGroup {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize)]
pub struct ControlPlaneChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub status_statistics_enabled: bool,
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
struct ControlPlaneChannelRow {
    id: Uuid,
    channel_group_id: Uuid,
    api_format: String,
    name: String,
    base_url: String,
    enabled: bool,
    status_statistics_enabled: bool,
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
impl From<ControlPlaneChannelRow> for ControlPlaneChannel {
    fn from(value: ControlPlaneChannelRow) -> Self {
        Self {
            id: value.id,
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            status_statistics_enabled: value.status_statistics_enabled,
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
pub struct ControlPlaneModelRule {
    pub id: Uuid,
    pub client_model: String,
    pub api_format: String,
    pub upstream_model_id: Uuid,
    pub upstream_model_enabled: bool,
    pub upstream_model: String,
    pub description: Option<String>,
    pub channel_group_ids: Vec<Uuid>,
    pub channel_ids: Vec<Uuid>,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneProxy {
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
pub struct ControlPlaneConfigTemplate {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub api_format: Option<String>,
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

    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        query_console_request_logs(&self.pool, Some(user_id), filter).await
    }

    pub async fn get_for_user(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        query_console_request_log(&self.pool, id, Some(user_id)).await
    }

    pub async fn list_all(
        &self,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        query_console_request_logs(&self.pool, None, filter).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        query_console_request_log(&self.pool, id, None).await
    }

    pub async fn channel_status(
        &self,
        window: ChannelStatusWindow,
    ) -> Result<ChannelStatusReport, RepositoryError> {
        let ended_at = Utc::now();
        let (started_at, ended_at) = window.range(ended_at);
        let tracked_channels = sqlx::query_as::<_, TrackedChannelRow>(
            "SELECT c.id,
                    c.channel_group_id,
                    g.name AS channel_group_name,
                    c.api_format::text AS api_format,
                    c.name,
                    c.enabled,
                    c.auto_disabled,
                    c.available_models
             FROM channels AS c
             JOIN channel_groups AS g ON g.id = c.channel_group_id
             WHERE c.status_statistics_enabled
             ORDER BY g.priority, g.name, c.name, c.id",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut overall_models = BTreeMap::<(String, String), ChannelStatusModelMetric>::new();
        let mut channel_indexes = BTreeMap::<Uuid, usize>::new();
        let mut channels =
            Vec::<ChannelStatusChannelBuilder>::with_capacity(tracked_channels.len());
        for channel in tracked_channels {
            let mut models = BTreeMap::new();
            for model in &channel.available_models {
                let key = (channel.api_format.clone(), model.clone());
                overall_models
                    .entry(key.clone())
                    .or_insert_with(|| empty_channel_status_metric(&key.0, &key.1));
                models
                    .entry(key.clone())
                    .or_insert_with(|| empty_channel_status_channel_model(&key.0, &key.1));
            }
            let index = channels.len();
            channel_indexes.insert(channel.id, index);
            channels.push(ChannelStatusChannelBuilder {
                id: channel.id,
                channel_group_id: channel.channel_group_id,
                channel_group_name: channel.channel_group_name,
                api_format: channel.api_format,
                name: channel.name,
                enabled: channel.enabled,
                auto_disabled: channel.auto_disabled,
                models,
            });
        }

        let overall_rows = sqlx::query_as::<_, StatusModelMetricRow>(
            "SELECT log.api_format::text AS api_format,
                    COALESCE(log.upstream_model, log.client_model) AS model,
                    count(*)::bigint AS request_count,
                    count(*) FILTER (WHERE log.outcome <> 'cancelled')::bigint
                        AS success_rate_request_count,
                    count(*) FILTER (WHERE log.outcome = 'succeeded')::bigint AS succeeded_count,
                    percentile_cont(0.9) WITHIN GROUP (ORDER BY log.ttft_ms::double precision)
                        FILTER (WHERE log.outcome = 'succeeded' AND log.ttft_ms IS NOT NULL)
                        AS p90_ttft_ms,
                    percentile_cont(0.5) WITHIN GROUP (
                        ORDER BY log.output_tokens_per_second::double precision
                    ) FILTER (
                        WHERE log.outcome = 'succeeded'
                          AND log.output_tokens_per_second IS NOT NULL
                    ) AS p50_tps
             FROM request_logs AS log
             JOIN channels AS channel ON channel.id = log.channel_id
             WHERE channel.status_statistics_enabled
               AND log.started_at >= $1
               AND log.started_at < $2
             GROUP BY log.api_format, COALESCE(log.upstream_model, log.client_model)
             ORDER BY log.api_format, COALESCE(log.upstream_model, log.client_model)",
        )
        .bind(started_at)
        .bind(ended_at)
        .fetch_all(&self.pool)
        .await?;
        for row in overall_rows {
            let key = (row.api_format.clone(), row.model.clone());
            overall_models.insert(key, row.into_metric());
        }

        let channel_rows = sqlx::query_as::<_, StatusChannelMetricRow>(
            "SELECT log.channel_id,
                    log.api_format::text AS api_format,
                    COALESCE(log.upstream_model, log.client_model) AS model,
                    count(*)::bigint AS request_count,
                    count(*) FILTER (WHERE log.outcome <> 'cancelled')::bigint
                        AS success_rate_request_count,
                    count(*) FILTER (WHERE log.outcome = 'succeeded')::bigint AS succeeded_count,
                    percentile_cont(0.9) WITHIN GROUP (ORDER BY log.ttft_ms::double precision)
                        FILTER (WHERE log.outcome = 'succeeded' AND log.ttft_ms IS NOT NULL)
                        AS p90_ttft_ms,
                    percentile_cont(0.5) WITHIN GROUP (
                        ORDER BY log.output_tokens_per_second::double precision
                    ) FILTER (
                        WHERE log.outcome = 'succeeded'
                          AND log.output_tokens_per_second IS NOT NULL
                    ) AS p50_tps
             FROM request_logs AS log
             JOIN channels AS channel ON channel.id = log.channel_id
             WHERE channel.status_statistics_enabled
               AND log.started_at >= $1
               AND log.started_at < $2
             GROUP BY log.channel_id, log.api_format,
                      COALESCE(log.upstream_model, log.client_model)
             ORDER BY log.channel_id, log.api_format,
                      COALESCE(log.upstream_model, log.client_model)",
        )
        .bind(started_at)
        .bind(ended_at)
        .fetch_all(&self.pool)
        .await?;
        for row in channel_rows {
            let Some(index) = channel_indexes.get(&row.channel_id).copied() else {
                continue;
            };
            let key = (row.api_format.clone(), row.model.clone());
            let metric = channels[index]
                .models
                .entry(key.clone())
                .or_insert_with(|| empty_channel_status_channel_model(&key.0, &key.1));
            metric.request_count = row.request_count;
            metric.success_rate = success_rate(row.success_rate_request_count, row.succeeded_count);
            metric.p90_ttft_ms = row.p90_ttft_ms;
            metric.p50_tps = row.p50_tps;
        }

        let history_rows = sqlx::query_as::<_, StatusBucketMetricRow>(
            "SELECT log.channel_id,
                    log.api_format::text AS api_format,
                    COALESCE(log.upstream_model, log.client_model) AS model,
                    to_timestamp(
                        floor(extract(epoch FROM log.started_at) / $3::double precision)
                        * $3::double precision
                    ) AS bucket_started_at,
                    count(*)::bigint AS request_count,
                    count(*) FILTER (WHERE log.outcome <> 'cancelled')::bigint
                        AS success_rate_request_count,
                    count(*) FILTER (WHERE log.outcome = 'succeeded')::bigint AS succeeded_count,
                    percentile_cont(0.9) WITHIN GROUP (ORDER BY log.ttft_ms::double precision)
                        FILTER (WHERE log.outcome = 'succeeded' AND log.ttft_ms IS NOT NULL)
                        AS p90_ttft_ms,
                    percentile_cont(0.5) WITHIN GROUP (
                        ORDER BY log.output_tokens_per_second::double precision
                    ) FILTER (
                        WHERE log.outcome = 'succeeded'
                          AND log.output_tokens_per_second IS NOT NULL
                    ) AS p50_tps
             FROM request_logs AS log
             JOIN channels AS channel ON channel.id = log.channel_id
             WHERE channel.status_statistics_enabled
               AND log.started_at >= $1
               AND log.started_at < $2
             GROUP BY log.channel_id, log.api_format,
                      COALESCE(log.upstream_model, log.client_model),
                      bucket_started_at
             ORDER BY log.channel_id, log.api_format,
                      COALESCE(log.upstream_model, log.client_model),
                      bucket_started_at",
        )
        .bind(started_at)
        .bind(ended_at)
        .bind(window.bucket_seconds())
        .fetch_all(&self.pool)
        .await?;
        for row in history_rows {
            let Some(index) = channel_indexes.get(&row.channel_id).copied() else {
                continue;
            };
            let key = (row.api_format.clone(), row.model.clone());
            channels[index]
                .models
                .entry(key.clone())
                .or_insert_with(|| empty_channel_status_channel_model(&key.0, &key.1))
                .history
                .push(ChannelStatusBucket {
                    started_at: row.bucket_started_at,
                    request_count: row.request_count,
                    success_rate: success_rate(row.success_rate_request_count, row.succeeded_count),
                    p90_ttft_ms: row.p90_ttft_ms,
                    p50_tps: row.p50_tps,
                });
        }

        Ok(ChannelStatusReport {
            window: window.as_str().into(),
            started_at,
            ended_at,
            bucket_seconds: window.bucket_seconds(),
            models: overall_models.into_values().collect(),
            channels: channels
                .into_iter()
                .map(ChannelStatusChannelBuilder::finish)
                .collect(),
        })
    }

    pub async fn cost_statistics(
        &self,
        filter: CostStatisticsFilter,
    ) -> Result<CostStatisticsReport, RepositoryError> {
        let duration = filter.ended_at.signed_duration_since(filter.started_at);
        if duration <= chrono::Duration::zero() || duration > filter.granularity.max_range() {
            return Err(RepositoryError::Validation);
        }

        let summary = sqlx::query_as::<_, CostSummaryRow>(
            "SELECT count(*)::bigint AS request_count,
                    count(cost_amount)::bigint AS priced_request_count,
                    COALESCE(
                        sum(COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)),
                        0
                    )::bigint AS total_tokens,
                    COALESCE(sum(cost_amount), 0) AS cost_amount
             FROM request_logs
             WHERE started_at >= $1
               AND started_at < $2
               AND ($3::uuid IS NULL OR user_id = $3)
               AND ($4::uuid IS NULL OR api_key_id = $4)",
        )
        .bind(filter.started_at)
        .bind(filter.ended_at)
        .bind(filter.user_id)
        .bind(filter.api_key_id)
        .fetch_one(&self.pool)
        .await?;

        let bucket_sql = format!(
            "SELECT {} AS bucket_started_at,
                    COALESCE(upstream_model, client_model) AS model,
                    api_format::text AS api_format,
                    count(*)::bigint AS request_count,
                    COALESCE(
                        sum(COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)),
                        0
                    )::bigint AS total_tokens,
                    COALESCE(sum(cost_amount), 0) AS cost_amount
             FROM request_logs
             WHERE started_at >= $1
               AND started_at < $2
               AND ($3::uuid IS NULL OR user_id = $3)
               AND ($4::uuid IS NULL OR api_key_id = $4)
             GROUP BY bucket_started_at,
                      COALESCE(upstream_model, client_model),
                      api_format
             ORDER BY bucket_started_at,
                      COALESCE(upstream_model, client_model),
                      api_format",
            filter.granularity.bucket_expression()
        );
        let bucket_rows = sqlx::query_as::<_, CostBucketMetricRow>(&bucket_sql)
            .bind(filter.started_at)
            .bind(filter.ended_at)
            .bind(filter.user_id)
            .bind(filter.api_key_id)
            .fetch_all(&self.pool)
            .await?;

        let model_rows = sqlx::query_as::<_, CostModelMetricRow>(
            "SELECT COALESCE(upstream_model, client_model) AS model,
                    api_format::text AS api_format,
                    count(*)::bigint AS request_count,
                    count(*) FILTER (WHERE outcome <> 'cancelled')::bigint
                        AS success_rate_request_count,
                    count(*) FILTER (WHERE outcome = 'succeeded')::bigint AS succeeded_count,
                    COALESCE(
                        sum(COALESCE(input_tokens, 0) + COALESCE(output_tokens, 0)),
                        0
                    )::bigint AS total_tokens,
                    COALESCE(sum(cost_amount), 0) AS cost_amount
             FROM request_logs
             WHERE started_at >= $1
               AND started_at < $2
               AND ($3::uuid IS NULL OR user_id = $3)
               AND ($4::uuid IS NULL OR api_key_id = $4)
             GROUP BY COALESCE(upstream_model, client_model), api_format
             ORDER BY COALESCE(upstream_model, client_model), api_format",
        )
        .bind(filter.started_at)
        .bind(filter.ended_at)
        .bind(filter.user_id)
        .bind(filter.api_key_id)
        .fetch_all(&self.pool)
        .await?;

        let duration_minutes = duration.num_milliseconds().max(1) as f64 / 60_000.0;
        Ok(CostStatisticsReport {
            started_at: filter.started_at,
            ended_at: filter.ended_at,
            granularity: filter.granularity.as_str().into(),
            summary: CostStatisticsSummary {
                request_count: summary.request_count,
                priced_request_count: summary.priced_request_count,
                total_tokens: summary.total_tokens,
                average_rpm: summary.request_count as f64 / duration_minutes,
                average_tpm: summary.total_tokens as f64 / duration_minutes,
                cost_amount: summary.cost_amount,
            },
            buckets: fold_cost_buckets(
                bucket_rows,
                filter.started_at,
                filter.ended_at,
                filter.granularity,
            ),
            models: fold_cost_models(model_rows),
        })
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

    /// Claims and applies one billable terminal log in a single transaction.
    ///
    /// The conditional `billed_at` update is the sole settlement claim. If a
    /// later account update fails, the transaction rolls back the claim too.
    /// This lets a durable recovery scan retry safely after worker restarts or
    /// transient database failures.
    pub async fn settle(
        &self,
        request_log_id: Uuid,
    ) -> Result<RequestLogSettlementOutcome, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let claimed = sqlx::query_as::<_, ClaimedRequestLog>(
            "UPDATE request_logs AS log
             SET billed_at = now()
             FROM api_keys AS key
             WHERE log.id = $1
               AND log.billed_at IS NULL
               AND log.cost_amount IS NOT NULL
               AND key.id = log.api_key_id
               AND key.user_id = log.user_id
             RETURNING log.id, log.user_id, log.api_key_id, log.cost_amount",
        )
        .bind(request_log_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some(claimed) = claimed else {
            let outcome = settlement_ineligibility(&mut transaction, request_log_id).await?;
            transaction.commit().await?;
            return Ok(outcome);
        };

        let balance_updated: Option<rust_decimal::Decimal> = sqlx::query_scalar(
            "UPDATE users
             SET balance_amount = balance_amount - $1
             WHERE id = $2
             RETURNING balance_amount",
        )
        .bind(claimed.cost_amount)
        .bind(claimed.user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if balance_updated.is_none() {
            return Err(RepositoryError::SettlementClaimInvalidated { id: request_log_id });
        }
        let quota_used_amount: Option<rust_decimal::Decimal> = sqlx::query_scalar(
            "UPDATE api_keys
             SET quota_used_amount = quota_used_amount + $1
             WHERE id = $2
               AND user_id = $3
             RETURNING quota_used_amount",
        )
        .bind(claimed.cost_amount)
        .bind(claimed.api_key_id)
        .bind(claimed.user_id)
        .fetch_optional(&mut *transaction)
        .await?;
        let quota_used_amount = quota_used_amount
            .ok_or(RepositoryError::SettlementClaimInvalidated { id: request_log_id })?;
        transaction.commit().await?;
        Ok(RequestLogSettlementOutcome::Settled {
            request_log_id: claimed.id,
            api_key_id: claimed.api_key_id,
            quota_used_amount,
        })
    }

    /// Reconciles a bounded oldest-first slice of durable, eligible logs.
    ///
    /// It intentionally excludes missing-cost and account-mismatch rows. Those
    /// rows remain visibly unbilled until their source facts are corrected
    /// instead of being retried forever as transient failures.
    pub async fn settle_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogSettlementOutcome>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        // Recovery is best-effort and must not monopolize the sole log worker
        // behind an administrative table lock. Immediate event persistence
        // keeps its existing longer timeout and remains the durable path.
        sqlx::query("SET LOCAL lock_timeout = '100ms'")
            .execute(&mut *transaction)
            .await?;
        let request_log_ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT log.id
             FROM request_logs AS log
             JOIN api_keys AS key
               ON key.id = log.api_key_id
              AND key.user_id = log.user_id
             WHERE log.billed_at IS NULL
               AND log.cost_amount IS NOT NULL
             ORDER BY log.completed_at, log.id
             LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        let mut outcomes = Vec::with_capacity(request_log_ids.len());
        for request_log_id in request_log_ids {
            outcomes.push(self.settle(request_log_id).await?);
        }
        Ok(outcomes)
    }
}

const CONSOLE_REQUEST_LOG_COLUMNS: &str = "id,started_at,completed_at,user_id,api_key_id,api_format::text AS api_format,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,outcome,response_status_code,streamed,ttft_ms,total_duration_ms,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,cost_amount,error_code,billed_at";

async fn query_console_request_log(
    pool: &PgPool,
    id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT {CONSOLE_REQUEST_LOG_COLUMNS} FROM request_logs WHERE id = "
    ));
    query.push_bind(id);
    if let Some(user_id) = owner_user_id {
        query.push(" AND user_id = ").push_bind(user_id);
    }
    query
        .build_query_as::<ConsoleRequestLog>()
        .fetch_optional(pool)
        .await
        .map_err(RepositoryError::from)
}

async fn query_console_request_logs(
    pool: &PgPool,
    owner_user_id: Option<Uuid>,
    filter: RequestLogFilter,
) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
    if filter
        .api_format
        .as_deref()
        .is_some_and(|value| !matches!(value, "open_ai_chat_completions" | "open_ai_responses"))
        || filter.outcome.as_deref().is_some_and(|value| {
            !matches!(value, "succeeded" | "failed" | "rejected" | "cancelled")
        })
        || filter
            .model
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
        || filter.started_after > filter.started_before
    {
        return Err(RepositoryError::Validation);
    }

    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT {CONSOLE_REQUEST_LOG_COLUMNS} FROM request_logs WHERE TRUE"
    ));
    if let Some(user_id) = owner_user_id {
        query.push(" AND user_id = ").push_bind(user_id);
    }
    if let Some(user_id) = filter.user_id {
        query.push(" AND user_id = ").push_bind(user_id);
    }
    if let Some(api_key_id) = filter.api_key_id {
        query.push(" AND api_key_id = ").push_bind(api_key_id);
    }
    if let Some(model) = filter.model {
        query
            .push(" AND (client_model = ")
            .push_bind(model.clone())
            .push(" OR upstream_model = ")
            .push_bind(model)
            .push(")");
    }
    if let Some(api_format) = filter.api_format {
        query.push(" AND api_format::text = ").push_bind(api_format);
    }
    if let Some(outcome) = filter.outcome {
        query.push(" AND outcome = ").push_bind(outcome);
    }
    if let Some(started_after) = filter.started_after {
        query.push(" AND started_at >= ").push_bind(started_after);
    }
    if let Some(started_before) = filter.started_before {
        query.push(" AND started_at <= ").push_bind(started_before);
    }
    if let Some(billed) = filter.billed {
        if billed {
            query.push(" AND billed_at IS NOT NULL");
        } else {
            query.push(" AND billed_at IS NULL");
        }
    }
    query
        .push(" ORDER BY started_at DESC, id DESC LIMIT ")
        .push_bind(filter.limit.clamp(1, 100));
    query
        .build_query_as::<ConsoleRequestLog>()
        .fetch_all(pool)
        .await
        .map_err(RepositoryError::from)
}

#[derive(FromRow)]
struct TrackedChannelRow {
    id: Uuid,
    channel_group_id: Uuid,
    channel_group_name: String,
    api_format: String,
    name: String,
    enabled: bool,
    auto_disabled: bool,
    available_models: Vec<String>,
}

#[derive(FromRow)]
struct StatusModelMetricRow {
    api_format: String,
    model: String,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

impl StatusModelMetricRow {
    fn into_metric(self) -> ChannelStatusModelMetric {
        ChannelStatusModelMetric {
            api_format: self.api_format,
            model: self.model,
            request_count: self.request_count,
            success_rate: success_rate(self.success_rate_request_count, self.succeeded_count),
            p90_ttft_ms: self.p90_ttft_ms,
            p50_tps: self.p50_tps,
        }
    }
}

#[derive(FromRow)]
struct StatusChannelMetricRow {
    channel_id: Uuid,
    api_format: String,
    model: String,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

#[derive(FromRow)]
struct StatusBucketMetricRow {
    channel_id: Uuid,
    api_format: String,
    model: String,
    bucket_started_at: DateTime<Utc>,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

struct ChannelStatusChannelBuilder {
    id: Uuid,
    channel_group_id: Uuid,
    channel_group_name: String,
    api_format: String,
    name: String,
    enabled: bool,
    auto_disabled: bool,
    models: BTreeMap<(String, String), ChannelStatusChannelModel>,
}

impl ChannelStatusChannelBuilder {
    fn finish(self) -> ChannelStatusChannel {
        ChannelStatusChannel {
            id: self.id,
            channel_group_id: self.channel_group_id,
            channel_group_name: self.channel_group_name,
            api_format: self.api_format,
            name: self.name,
            enabled: self.enabled,
            auto_disabled: self.auto_disabled,
            models: self
                .models
                .into_values()
                .map(|mut model| {
                    model.history.sort_by_key(|bucket| bucket.started_at);
                    model
                })
                .collect(),
        }
    }
}

fn empty_channel_status_metric(api_format: &str, model: &str) -> ChannelStatusModelMetric {
    ChannelStatusModelMetric {
        api_format: api_format.into(),
        model: model.into(),
        request_count: 0,
        success_rate: None,
        p90_ttft_ms: None,
        p50_tps: None,
    }
}

fn empty_channel_status_channel_model(api_format: &str, model: &str) -> ChannelStatusChannelModel {
    ChannelStatusChannelModel {
        api_format: api_format.into(),
        model: model.into(),
        request_count: 0,
        success_rate: None,
        p90_ttft_ms: None,
        p50_tps: None,
        history: Vec::new(),
    }
}

fn success_rate(eligible_request_count: i64, succeeded_count: i64) -> Option<f64> {
    (eligible_request_count > 0).then_some(succeeded_count as f64 / eligible_request_count as f64)
}

#[derive(FromRow)]
struct CostSummaryRow {
    request_count: i64,
    priced_request_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct CostBucketMetricRow {
    bucket_started_at: DateTime<Utc>,
    model: String,
    api_format: String,
    request_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct CostModelMetricRow {
    model: String,
    api_format: String,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(Default)]
struct CostBucketBuilder {
    request_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
    models: BTreeMap<(String, String), CostBucketModelBuilder>,
}

#[derive(Default)]
struct CostBucketModelBuilder {
    request_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(Default)]
struct CostModelBuilder {
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

fn fold_cost_buckets(
    rows: Vec<CostBucketMetricRow>,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    granularity: StatisticsGranularity,
) -> Vec<CostStatisticsBucket> {
    let mut buckets = BTreeMap::<DateTime<Utc>, CostBucketBuilder>::new();
    let bucket_seconds = granularity.bucket_seconds();
    let aligned_started_at = DateTime::from_timestamp(
        started_at.timestamp().div_euclid(bucket_seconds) * bucket_seconds,
        0,
    )
    .unwrap_or(started_at);
    let mut bucket_started_at = aligned_started_at;
    while bucket_started_at < ended_at {
        buckets.entry(bucket_started_at).or_default();
        let Some(next) =
            bucket_started_at.checked_add_signed(chrono::Duration::seconds(bucket_seconds))
        else {
            break;
        };
        bucket_started_at = next;
    }
    for row in rows {
        let bucket = buckets.entry(row.bucket_started_at).or_default();
        bucket.request_count = bucket.request_count.saturating_add(row.request_count);
        bucket.total_tokens = bucket.total_tokens.saturating_add(row.total_tokens);
        bucket.cost_amount += row.cost_amount;
        let model = bucket
            .models
            .entry((row.api_format, row.model))
            .or_default();
        model.request_count = model.request_count.saturating_add(row.request_count);
        model.total_tokens = model.total_tokens.saturating_add(row.total_tokens);
        model.cost_amount += row.cost_amount;
    }

    buckets
        .into_iter()
        .map(|(started_at, bucket)| CostStatisticsBucket {
            started_at,
            request_count: bucket.request_count,
            total_tokens: bucket.total_tokens,
            cost_amount: bucket.cost_amount,
            models: bucket
                .models
                .into_iter()
                .map(|((api_format, model), metric)| CostStatisticsBucketModel {
                    api_format,
                    model,
                    request_count: metric.request_count,
                    total_tokens: metric.total_tokens,
                    cost_amount: metric.cost_amount,
                })
                .collect(),
        })
        .collect()
}

fn fold_cost_models(rows: Vec<CostModelMetricRow>) -> Vec<CostStatisticsModel> {
    let mut models = BTreeMap::<(String, String), CostModelBuilder>::new();
    for row in rows {
        let model = models.entry((row.api_format, row.model)).or_default();
        model.request_count = model.request_count.saturating_add(row.request_count);
        model.success_rate_request_count = model
            .success_rate_request_count
            .saturating_add(row.success_rate_request_count);
        model.succeeded_count = model.succeeded_count.saturating_add(row.succeeded_count);
        model.total_tokens = model.total_tokens.saturating_add(row.total_tokens);
        model.cost_amount += row.cost_amount;
    }

    models
        .into_iter()
        .map(|((api_format, model), metric)| CostStatisticsModel {
            api_format,
            model,
            request_count: metric.request_count,
            total_tokens: metric.total_tokens,
            success_rate: success_rate(metric.success_rate_request_count, metric.succeeded_count),
            cost_amount: metric.cost_amount,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogInsertOutcome {
    Inserted,
    ExactDuplicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestLogSettlementOutcome {
    Settled {
        request_log_id: Uuid,
        api_key_id: Uuid,
        quota_used_amount: rust_decimal::Decimal,
    },
    AlreadyBilled,
    NotBillable,
    AccountMismatch,
    NotFound,
}

#[derive(FromRow)]
struct ClaimedRequestLog {
    id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct SettlementEligibility {
    billed_at: Option<DateTime<Utc>>,
    cost_amount: Option<rust_decimal::Decimal>,
    api_key_user_id: Option<Uuid>,
    user_id: Uuid,
}

async fn settlement_ineligibility(
    transaction: &mut Transaction<'_, Postgres>,
    request_log_id: Uuid,
) -> Result<RequestLogSettlementOutcome, RepositoryError> {
    let eligibility = sqlx::query_as::<_, SettlementEligibility>(
        "SELECT log.billed_at,
                log.cost_amount,
                key.user_id AS api_key_user_id,
                log.user_id
         FROM request_logs AS log
         LEFT JOIN api_keys AS key ON key.id = log.api_key_id
         WHERE log.id = $1",
    )
    .bind(request_log_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(eligibility) = eligibility else {
        return Ok(RequestLogSettlementOutcome::NotFound);
    };
    if eligibility.billed_at.is_some() {
        return Ok(RequestLogSettlementOutcome::AlreadyBilled);
    }
    if eligibility.cost_amount.is_none() {
        return Ok(RequestLogSettlementOutcome::NotBillable);
    }
    if eligibility.api_key_user_id != Some(eligibility.user_id) {
        return Ok(RequestLogSettlementOutcome::AccountMismatch);
    }
    // A concurrent claimer either commits and is observed as `AlreadyBilled`,
    // or rolls back and leaves a later recovery pass to claim the row.
    Ok(RequestLogSettlementOutcome::NotBillable)
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

fn generate_api_key_secret() -> String {
    // Two UUIDv4 values provide 32 random bytes in a transport-safe form.
    format!("sk-{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
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
        let model_rules = sqlx::query_as::<_, ModelRuleRecord>("SELECT r.id, r.client_model, r.api_format::text AS api_format, r.upstream_model_id, m.enabled AS upstream_model_enabled, m.currency AS upstream_model_currency, m.price_unit_tokens, m.price_effective_at, m.input_unit_price, m.cached_input_unit_price, m.cache_write_unit_price, m.output_unit_price, m.source_model_id AS upstream_model, r.channel_group_ids, r.channel_ids, r.enabled FROM model_rules r JOIN models m ON m.id = r.upstream_model_id ORDER BY r.id").fetch_all(&mut **transaction).await?;
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

    pub async fn active_admin_exists(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<bool, RepositoryError> {
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND status = 'active' AND role = 'admin')",
        )
        .bind(id)
        .fetch_one(&mut **transaction)
        .await?)
    }

    pub async fn control_plane_lists(&self) -> Result<ControlPlaneLists, RepositoryError> {
        let users = sqlx::query_as::<_, ControlPlaneUser>(
            "SELECT id,email,display_name,role,status,default_api_key_policy_id,balance_amount,created_at,updated_at FROM users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let models = sqlx::query_as::<_, ControlPlaneModel>("SELECT id,source_model_id,display_name,provider_name,enabled,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,last_synced_at,created_at,updated_at FROM models ORDER BY id").fetch_all(&self.pool).await?;
        let api_keys = sqlx::query_as::<_, ControlPlaneApiKey>("SELECT k.id, k.user_id, u.status AS user_status, k.name, k.secret_value AS secret, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.requests_per_minute, k.tokens_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount, k.updated_at FROM api_keys k JOIN users u ON u.id=k.user_id ORDER BY k.id").fetch_all(&self.pool).await?;
        let api_key_policies = sqlx::query_as::<_, ControlPlaneApiKeyPolicy>("SELECT id,name,allowed_api_formats::text[] AS allowed_api_formats,permissions,allowed_group_ids,requests_per_minute,max_concurrent_requests,quota_limit_amount,max_active_keys,enabled,created_at,updated_at FROM api_key_policies ORDER BY id").fetch_all(&self.pool).await?;
        let channel_groups = sqlx::query_as::<_, ControlPlaneChannelGroup>("SELECT id,name,api_format::text AS api_format,priority,selection_strategy,enabled,updated_at FROM channel_groups ORDER BY id").fetch_all(&self.pool).await?;
        let channels = sqlx::query_as::<_, ControlPlaneChannelRow>("SELECT id,channel_group_id,api_format::text AS api_format,name,base_url,enabled,status_statistics_enabled,auto_disabled,auto_disabled_reason,weight,proxy_id,config_template_id,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,(upstream_api_key IS NOT NULL) AS upstream_credential_configured,available_models,created_at,updated_at FROM channels ORDER BY id").fetch_all(&self.pool).await?;
        let model_rules = sqlx::query_as::<_, ControlPlaneModelRule>("SELECT r.id,r.client_model,r.api_format::text AS api_format,r.upstream_model_id,m.enabled AS upstream_model_enabled,m.source_model_id AS upstream_model,r.description,r.channel_group_ids,r.channel_ids,r.enabled,r.updated_at FROM model_rules r JOIN models m ON m.id=r.upstream_model_id ORDER BY r.id").fetch_all(&self.pool).await?;
        let proxies = sqlx::query_as::<_, ControlPlaneProxy>("SELECT id,name,regexp_replace(regexp_replace(proxy_url, '^([^:/?#]+://)[^/?#]*@', E'\\1'), '[?#].*$', '') AS proxy_url,no_proxy_hosts,enabled,(username IS NOT NULL OR password IS NOT NULL) AS credential_configured,created_at,updated_at FROM proxies ORDER BY id").fetch_all(&self.pool).await?;
        let config_templates = sqlx::query_as::<_, ControlPlaneConfigTemplate>("SELECT id,name,description,document->>'api_format' AS api_format,enabled,created_at,updated_at FROM config_templates ORDER BY id").fetch_all(&self.pool).await?;
        Ok(ControlPlaneLists {
            users,
            models,
            api_keys,
            api_key_policies,
            channel_groups,
            channels: channels.into_iter().map(Into::into).collect(),
            model_rules,
            proxies,
            config_templates,
        })
    }

    pub async fn audit_logs(&self, limit: i64) -> Result<Vec<ConsoleAuditLog>, RepositoryError> {
        sqlx::query_as::<_, ConsoleAuditLog>(
            "SELECT id,occurred_at,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id,reason FROM audit_logs ORDER BY occurred_at DESC,id DESC LIMIT $1",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn own_api_keys(&self, user_id: Uuid) -> Result<Vec<ConsoleApiKey>, RepositoryError> {
        sqlx::query_as::<_, ConsoleApiKey>(
            "SELECT id,name,secret_value AS secret,status,expires_at,allowed_api_formats::text[] AS allowed_api_formats, \
                    permissions,allowed_group_ids,requests_per_minute,max_concurrent_requests, \
                    quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE user_id=$1 ORDER BY created_at DESC,id DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn own_api_key(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleApiKey>, RepositoryError> {
        sqlx::query_as::<_, ConsoleApiKey>(
            "SELECT id,name,secret_value AS secret,status,expires_at,allowed_api_formats::text[] AS allowed_api_formats, \
                    permissions,allowed_group_ids,requests_per_minute,max_concurrent_requests, \
                    quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE id=$1 AND user_id=$2",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn create_own_api_key(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<MutationResult, RepositoryError> {
        let policy = sqlx::query_as::<_, SelfApiKeyPolicy>(
            "SELECT p.allowed_api_formats::text[] AS allowed_api_formats,p.permissions, \
                    p.allowed_group_ids,p.requests_per_minute,p.max_concurrent_requests, \
                    p.quota_limit_amount,p.max_active_keys,p.enabled \
             FROM users AS u \
             JOIN api_key_policies AS p ON p.id=u.default_api_key_policy_id \
             WHERE u.id=$1 AND u.status='active' FOR UPDATE OF u",
        )
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::DefaultApiKeyPolicyRequired)?;
        if !policy.enabled {
            return Err(RepositoryError::DefaultApiKeyPolicyDisabled);
        }
        let active_key_count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM api_keys \
             WHERE user_id=$1 AND status IN ('active','disabled')",
        )
        .bind(user_id)
        .fetch_one(&mut **transaction)
        .await?;
        if active_key_count >= i64::from(policy.max_active_keys) {
            return Err(RepositoryError::ApiKeyLimitReached);
        }
        if input.name.trim().is_empty() {
            return Err(RepositoryError::Validation);
        }
        let id = Uuid::new_v4();
        let secret = generate_api_key_secret();
        let updated_at = sqlx::query_scalar(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,expires_at,allowed_api_formats,permissions, \
              allowed_group_ids,requests_per_minute,max_concurrent_requests,quota_limit_amount) \
             VALUES ($1,$2,$3,$4,'active',$5,$6::api_format[],$7,$8,$9,$10,$11) \
             RETURNING updated_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&secret)
        .bind(input.expires_at)
        .bind(&policy.allowed_api_formats)
        .bind(&policy.permissions)
        .bind(&policy.allowed_group_ids)
        .bind(policy.requests_per_minute)
        .bind(policy.max_concurrent_requests)
        .bind(policy.quota_limit_amount)
        .fetch_one(&mut **transaction)
        .await?;
        Ok(MutationResult {
            id,
            object_type: "api_key",
            action: "self_create",
            before_redacted: json!({}),
            after_redacted: key_audit(transaction, id).await?,
            created_secret: Some(secret),
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn update_own_api_key(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        if input.name.trim().is_empty() || !matches!(input.status.as_str(), "active" | "disabled") {
            return Err(RepositoryError::Validation);
        }
        let before = key_audit_for_user(transaction, id, user_id).await?;
        let updated_at = sqlx::query_scalar(
            "UPDATE api_keys SET name=$3,status=$4,expires_at=$5 \
             WHERE id=$1 AND user_id=$2 AND updated_at=$6 AND status <> 'revoked' \
             RETURNING updated_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&input.status)
        .bind(input.expires_at)
        .bind(expected_updated_at)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        Ok(MutationResult {
            id,
            object_type: "api_key",
            action: "self_update",
            before_redacted: before,
            after_redacted: key_audit_for_user(transaction, id, user_id).await?,
            created_secret: None,
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn revoke_own_api_key(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<MutationResult, RepositoryError> {
        if reason.trim().is_empty() {
            return Err(RepositoryError::Validation);
        }
        let before = key_audit_for_user(transaction, id, user_id).await?;
        let updated_at = sqlx::query_scalar(
            "UPDATE api_keys SET status='revoked' \
             WHERE id=$1 AND user_id=$2 AND status <> 'revoked' RETURNING updated_at",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        Ok(MutationResult {
            id,
            object_type: "api_key",
            action: "self_revoke",
            before_redacted: before,
            after_redacted: key_audit_for_user(transaction, id, user_id).await?,
            created_secret: None,
            reason: Some(reason),
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn apply_control_plane_mutation(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        mutation: ControlPlaneMutation,
    ) -> Result<MutationResult, RepositoryError> {
        match mutation {
            ControlPlaneMutation::CreateUser(input) => {
                user_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateUser {
                id,
                input,
                expected_updated_at,
            } => user_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateModel(input) => {
                model_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateModel {
                id,
                input,
                expected_updated_at,
            } => model_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateApiKey(input) => {
                let id = Uuid::new_v4();
                let secret = generate_api_key_secret();
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
            ControlPlaneMutation::CreateApiKeyPolicy(input) => {
                api_key_policy_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateApiKeyPolicy {
                id,
                input,
                expected_updated_at,
            } => {
                api_key_policy_insert(transaction, id, input, false, Some(expected_updated_at))
                    .await
            }
            ControlPlaneMutation::UpdateApiKey {
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
            ControlPlaneMutation::RevokeApiKey { id, reason } => {
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
            ControlPlaneMutation::CreateGroup(input) => {
                group_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateGroup {
                id,
                input,
                expected_updated_at,
            } => group_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateChannel(input) => {
                channel_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateChannel {
                id,
                input,
                expected_updated_at,
            } => channel_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateRule(input) => {
                rule_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateRule {
                id,
                input,
                expected_updated_at,
            } => rule_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateProxy(input) => {
                proxy_insert(transaction, Uuid::new_v4(), input).await
            }
            ControlPlaneMutation::UpdateProxy {
                id,
                input,
                expected_updated_at,
            } => proxy_update(transaction, id, input, expected_updated_at).await,
            ControlPlaneMutation::CreateConfigTemplate(input) => {
                config_template_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateConfigTemplate {
                id,
                input,
                expected_updated_at,
            } => {
                config_template_insert(transaction, id, input, false, Some(expected_updated_at))
                    .await
            }
        }
    }

    pub async fn model_source_ids(&self) -> Result<Vec<String>, RepositoryError> {
        sqlx::query_scalar("SELECT source_model_id FROM models ORDER BY source_model_id")
            .fetch_all(&self.pool)
            .await
            .map_err(RepositoryError::from)
    }

    /// Applies explicitly selected catalog entries. Existing source-model IDs
    /// receive a price refresh; absent IDs are imported as new local models.
    pub async fn apply_catalog_models(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        inputs: Vec<SyncedModelInput>,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        let synced_at = Utc::now();
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            let existing_id = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM models WHERE source_model_id=$1 FOR UPDATE",
            )
            .bind(&input.source_model_id)
            .fetch_optional(&mut **transaction)
            .await?;
            results.push(match existing_id {
                Some(id) => sync_model_price(transaction, id, input, synced_at).await?,
                None => import_model(transaction, input, synced_at).await?,
            });
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
        sqlx::query("INSERT INTO audit_logs (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id,reason) VALUES ($1,$2,'user','admin',$3,$4,$5,$6,$7,$8,$9)")
            .bind(Uuid::new_v4()).bind(actor).bind(mutation.action).bind(mutation.object_type).bind(mutation.id).bind(&mutation.before_redacted).bind(&mutation.after_redacted).bind(correlation_id.to_string()).bind(&mutation.reason).execute(&mut **transaction).await?;
        Ok(())
    }
    pub async fn insert_self_audit(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor: Uuid,
        mutation: &MutationResult,
        correlation_id: Uuid,
    ) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO audit_logs (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id,reason) VALUES ($1,$2,'user','user',$3,$4,$5,$6,$7,$8,$9)")
            .bind(Uuid::new_v4()).bind(actor).bind(mutation.action).bind(mutation.object_type).bind(mutation.id).bind(&mutation.before_redacted).bind(&mutation.after_redacted).bind(correlation_id.to_string()).bind(&mutation.reason).execute(&mut **transaction).await?;
        Ok(())
    }
    pub async fn insert_manual_reload_audit(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor: Uuid,
        correlation_id: Uuid,
    ) -> Result<(), RepositoryError> {
        sqlx::query("INSERT INTO audit_logs (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted,correlation_id) VALUES ($1,$2,'user','admin','reload','runtime_config',$3,'{}','{}',$4)")
            .bind(Uuid::new_v4()).bind(actor).bind(Uuid::nil()).bind(correlation_id.to_string()).execute(&mut **transaction).await?;
        Ok(())
    }
}

#[derive(FromRow)]
struct SelfApiKeyPolicy {
    allowed_api_formats: Vec<String>,
    permissions: Vec<String>,
    allowed_group_ids: Option<Vec<Uuid>>,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    max_active_keys: i32,
    enabled: bool,
}

async fn key_audit_for_user(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    user_id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'user_id',user_id,'name',name,'status',status,'expires_at',expires_at,'allowed_api_formats',allowed_api_formats,'permissions',permissions,'allowed_group_ids',allowed_group_ids,'requests_per_minute',requests_per_minute,'tokens_per_minute',tokens_per_minute,'max_concurrent_requests',max_concurrent_requests,'quota_limit_amount',quota_limit_amount,'quota_used_amount',quota_used_amount,'created_at',created_at,'updated_at',updated_at) FROM api_keys WHERE id=$1 AND user_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await?;
    value.ok_or(RepositoryError::NotFound)
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
        "SELECT json_build_object('id',id,'email',email,'display_name',display_name,'role',role,'status',status,'default_api_key_policy_id',default_api_key_policy_id,'balance_amount',balance_amount,'created_at',created_at,'updated_at',updated_at) FROM users WHERE id=$1 FOR UPDATE",
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
        "SELECT json_build_object('id',id,'source_model_id',source_model_id,'display_name',display_name,'provider_name',provider_name,'enabled',enabled,'price_unit_tokens',price_unit_tokens,'input_unit_price',input_unit_price,'cached_input_unit_price',cached_input_unit_price,'cache_write_unit_price',cache_write_unit_price,'output_unit_price',output_unit_price,'price_effective_at',price_effective_at,'last_synced_at',last_synced_at,'created_at',created_at,'updated_at',updated_at) FROM models WHERE id=$1 FOR UPDATE",
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
    // Keep this allowlist aligned with `ControlPlaneChannel`: channel documents are
    // intentionally opaque today and must never leave the database through
    // either management responses or audit snapshots.
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'channel_group_id',channel_group_id,'api_format',api_format,'name',name,'base_url',base_url,'enabled',enabled,'status_statistics_enabled',status_statistics_enabled,'auto_disabled',auto_disabled,'auto_disabled_reason',auto_disabled_reason,'weight',weight,'proxy_id',proxy_id,'config_template_id',config_template_id,'connect_timeout_ms',connect_timeout_ms,'response_header_timeout_ms',response_header_timeout_ms,'stream_idle_timeout_ms',stream_idle_timeout_ms,'upstream_auth_kind',upstream_auth_kind,'upstream_auth_header_name',upstream_auth_header_name,'upstream_credential_configured',(upstream_api_key IS NOT NULL),'available_models',available_models,'created_at',created_at,'updated_at',updated_at) FROM channels WHERE id=$1 FOR UPDATE",
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
async fn api_key_policy_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'name',name,'allowed_api_formats',allowed_api_formats,'permissions',permissions,'allowed_group_ids',allowed_group_ids,'requests_per_minute',requests_per_minute,'max_concurrent_requests',max_concurrent_requests,'quota_limit_amount',quota_limit_amount,'max_active_keys',max_active_keys,'enabled',enabled,'created_at',created_at,'updated_at',updated_at) FROM api_key_policies WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    value.ok_or(RepositoryError::NotFound)
}

async fn api_key_policy_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: ApiKeyPolicyInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input.name.trim().is_empty()
        || input.max_active_keys <= 0
        || input.allowed_api_formats.is_empty()
        || input.permissions.is_empty()
    {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        api_key_policy_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar(
            "INSERT INTO api_key_policies \
             (id,name,allowed_api_formats,permissions,allowed_group_ids,requests_per_minute,max_concurrent_requests,quota_limit_amount,max_active_keys,enabled) \
             VALUES ($1,$2,$3::api_format[],$4,$5,$6,$7,$8,$9,$10) RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.allowed_api_formats)
        .bind(&input.permissions)
        .bind(&input.allowed_group_ids)
        .bind(input.requests_per_minute)
        .bind(input.max_concurrent_requests)
        .bind(input.quota_limit_amount)
        .bind(input.max_active_keys)
        .bind(input.enabled)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE api_key_policies \
             SET name=$2,allowed_api_formats=$3::api_format[],permissions=$4,allowed_group_ids=$5, \
                 requests_per_minute=$6,max_concurrent_requests=$7,quota_limit_amount=$8, \
                 max_active_keys=$9,enabled=$10 \
             WHERE id=$1 AND updated_at=$11 RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.allowed_api_formats)
        .bind(&input.permissions)
        .bind(&input.allowed_group_ids)
        .bind(input.requests_per_minute)
        .bind(input.max_concurrent_requests)
        .bind(input.quota_limit_amount)
        .bind(input.max_active_keys)
        .bind(input.enabled)
        .bind(expected_updated_at.expect("PUT version"))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "api_key_policy",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: api_key_policy_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
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
    let invalidates_sessions = !create
        && (before["email"].as_str() != input.email.as_deref()
            || before["role"].as_str() != Some(input.role.as_str())
            || before["status"].as_str() != Some(input.status.as_str()));
    let updated_at = if create {
        sqlx::query_scalar(
            "INSERT INTO users (id,email,display_name,role,status,balance_amount,default_api_key_policy_id) VALUES ($1,$2,$3,$4,$5,$6,$7) RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.email)
        .bind(&input.display_name)
        .bind(&input.role)
        .bind(&input.status)
        .bind(input.balance_amount)
        .bind(input.default_api_key_policy_id)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE users SET email=$2,display_name=$3,role=$4,status=$5,balance_amount=$6,default_api_key_policy_id=$7, \
             auth_version=auth_version+CASE WHEN $8 THEN 1 ELSE 0 END \
             WHERE id=$1 AND updated_at=$9 RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.email)
        .bind(&input.display_name)
        .bind(&input.role)
        .bind(&input.status)
        .bind(input.balance_amount)
        .bind(input.default_api_key_policy_id)
        .bind(invalidates_sessions)
        .bind(expected_updated_at.expect("PUT version"))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    if invalidates_sessions {
        sqlx::query(
            "UPDATE user_sessions SET revoked_at=now() WHERE user_id=$1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    }
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
        sqlx::query_scalar("INSERT INTO models (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,source_payload) VALUES ($1,$2,$3,$4,$5,'USD',$6,$7,$8,$9,$10,$11,$12) RETURNING updated_at")
            .bind(id)
            .bind(&input.source_model_id)
            .bind(&input.display_name)
            .bind(&input.provider_name)
            .bind(input.enabled)
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
        sqlx::query_scalar("UPDATE models SET source_model_id=$2,display_name=$3,provider_name=$4,enabled=$5,currency='USD',price_unit_tokens=$6,input_unit_price=$7,cached_input_unit_price=$8,cache_write_unit_price=$9,output_unit_price=$10,price_effective_at=$11,source_payload=CASE WHEN $12 THEN $13 ELSE source_payload END WHERE id=$1 AND updated_at=$14 RETURNING updated_at")
            .bind(id)
            .bind(&input.source_model_id)
            .bind(&input.display_name)
            .bind(&input.provider_name)
            .bind(input.enabled)
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
/// Refreshes only catalog-owned price facts for a local source model. Display
/// name, provider label, and enabled state remain administrator-managed.
async fn sync_model_price(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: SyncedModelInput,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let before = model_audit(transaction, id).await?;
    let updated_at = sqlx::query_scalar(
        "UPDATE models
         SET currency='USD',
             price_unit_tokens=1000000,
             input_unit_price=$3,
             cached_input_unit_price=$4,
             cache_write_unit_price=$5,
             output_unit_price=$6,
             price_effective_at=$7,
             source_payload=$8,
             last_synced_at=$7
         WHERE id=$1 AND source_model_id=$2
         RETURNING updated_at",
    )
    .bind(id)
    .bind(&input.source_model_id)
    .bind(input.input_unit_price)
    .bind(input.cached_input_unit_price)
    .bind(input.cache_write_unit_price)
    .bind(input.output_unit_price)
    .bind(synced_at)
    .bind(&input.source_payload)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "model",
        action: "price_sync",
        before_redacted: before,
        after_redacted: model_audit(transaction, id).await?,
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
    if input
        .override_document
        .as_ref()
        .is_some_and(|document| document.as_object().is_none())
        || !is_empty_document(&input.health_check)
    {
        return Err(RepositoryError::Validation);
    }
    if matches!(input.upstream_api_key, Some(None)) && input.upstream_auth_kind != "none" {
        return Err(RepositoryError::Validation);
    }
    let override_document_present = input.override_document.is_some();
    let override_document = input.override_document.unwrap_or_else(empty_object);
    let before = if create {
        json!({})
    } else {
        channel_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,weight,proxy_id,config_template_id,override_document,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,upstream_api_key,available_models,health_check,status_statistics_enabled) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19) RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.proxy_id).bind(input.config_template_id).bind(&override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.health_check).bind(input.status_statistics_enabled).fetch_one(&mut **transaction).await?
    } else {
        let credential_present = input.upstream_api_key.is_some();
        sqlx::query_scalar("UPDATE channels SET channel_group_id=$2,api_format=$3::api_format,name=$4,base_url=$5,enabled=$6,weight=$7,proxy_id=$8,config_template_id=$9,override_document=CASE WHEN $10 THEN $11 ELSE override_document END,connect_timeout_ms=$12,response_header_timeout_ms=$13,stream_idle_timeout_ms=$14,upstream_auth_kind=$15,upstream_auth_header_name=$16,upstream_api_key=CASE WHEN $17 THEN $18 ELSE upstream_api_key END,available_models=$19,health_check=$20,status_statistics_enabled=$21 WHERE id=$1 AND updated_at=$22 RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.proxy_id).bind(input.config_template_id).bind(override_document_present).bind(&override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(credential_present).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.health_check).bind(input.status_statistics_enabled).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
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
        sqlx::query_scalar("INSERT INTO model_rules (id,client_model,api_format,upstream_model_id,description,channel_group_ids,channel_ids,enabled) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7,$8) RETURNING updated_at").bind(id).bind(&input.client_model).bind(&input.api_format).bind(input.upstream_model_id).bind(&input.description).bind(&input.channel_group_ids).bind(&input.channel_ids).bind(input.enabled).fetch_one(&mut **transaction).await?
    } else {
        sqlx::query_scalar("UPDATE model_rules SET client_model=$2,api_format=$3::api_format,upstream_model_id=$4,description=$5,channel_group_ids=$6,channel_ids=$7,enabled=$8 WHERE id=$1 AND updated_at=$9 RETURNING updated_at").bind(id).bind(&input.client_model).bind(&input.api_format).bind(input.upstream_model_id).bind(&input.description).bind(&input.channel_group_ids).bind(&input.channel_ids).bind(input.enabled).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
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
    input: impl Into<ConfigTemplateMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if create && input.document.is_none() {
        return Err(RepositoryError::Validation);
    }
    let document_present = input.document.is_some();
    let document = input.document.unwrap_or_else(empty_object);
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
            .bind(&document)
            .bind(input.enabled)
            .fetch_one(&mut **transaction)
            .await?
    } else {
        sqlx::query_scalar("UPDATE config_templates SET name=$2,description=$3,document=CASE WHEN $4 THEN $5 ELSE document END,enabled=$6 WHERE id=$1 AND updated_at=$7 RETURNING updated_at")
            .bind(id)
            .bind(&input.name)
            .bind(&input.description)
            .bind(document_present)
            .bind(&document)
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
    #[error("request log settlement claim became ineligible before account updates")]
    SettlementClaimInvalidated { id: Uuid },
    #[error("requested record was not found or cannot be changed")]
    NotFound,
    #[error("management record version conflicts with the current version")]
    Conflict,
    #[error("management input is invalid")]
    Validation,
    #[error("the user has no default API key policy")]
    DefaultApiKeyPolicyRequired,
    #[error("the user's default API key policy is disabled")]
    DefaultApiKeyPolicyDisabled,
    #[error("the user's active API key limit has been reached")]
    ApiKeyLimitReached,
}
