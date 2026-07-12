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
            .field("override_document", &self.override_document)
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
    pub api_keys: Vec<AdminApiKey>,
    pub channel_groups: Vec<AdminChannelGroup>,
    pub channels: Vec<AdminChannel>,
    pub model_rules: Vec<AdminModelRule>,
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
        let started_at = normalize_timestamp(event.started_at);
        let completed_at = normalize_timestamp(event.completed_at);
        let inserted = sqlx::query_scalar::<_, Uuid>("INSERT INTO request_logs (id, started_at, completed_at, user_id, api_key_id, api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, model_id, error_code) VALUES ($1, $2, $3, $4, $5, $6::api_format, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18) ON CONFLICT (id) DO NOTHING RETURNING id")
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
            .bind(event.model_id)
            .bind(event.error_code)
            .fetch_optional(&self.pool)
            .await?;
        if inserted.is_some() {
            return Ok(RequestLogInsertOutcome::Inserted);
        }

        let existing = sqlx::query_as::<_, StoredRequestLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, model_id, error_code FROM request_logs WHERE id = $1")
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
    model_id: Option<Uuid>,
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
            && self.model_id == event.model_id
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
        let model_rules = sqlx::query_as::<_, ModelRuleRecord>("SELECT r.id, r.client_model, r.api_format::text AS api_format, r.model_id, m.enabled AS model_enabled, r.upstream_model, r.channel_group_ids, r.channel_ids, r.enabled FROM model_rules r JOIN models m ON m.id = r.model_id ORDER BY r.id").fetch_all(&mut **transaction).await?;
        let groups = sqlx::query_as::<_, ChannelGroupRecord>("SELECT id, name, api_format::text AS api_format, priority, selection_strategy, enabled FROM channel_groups ORDER BY id").fetch_all(&mut **transaction).await?;
        let channels = sqlx::query_as::<_, ChannelRecord>("SELECT id, channel_group_id, api_format::text AS api_format, name, base_url, enabled, auto_disabled, weight, proxy_id, config_template_id, override_document, connect_timeout_ms, response_header_timeout_ms, stream_idle_timeout_ms, upstream_auth_kind, upstream_auth_header_name, upstream_api_key, available_models, health_check FROM channels ORDER BY id").fetch_all(&mut **transaction).await?;
        Ok(ControlPlaneRecords {
            api_keys,
            model_rules,
            groups,
            channels,
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
        let api_keys = sqlx::query_as::<_, AdminApiKey>("SELECT k.id, k.user_id, u.status AS user_status, k.name, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.requests_per_minute, k.tokens_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount, k.updated_at FROM api_keys k JOIN users u ON u.id=k.user_id ORDER BY k.id").fetch_all(&self.pool).await?;
        let channel_groups = sqlx::query_as::<_, AdminChannelGroup>("SELECT id,name,api_format::text AS api_format,priority,selection_strategy,enabled,updated_at FROM channel_groups ORDER BY id").fetch_all(&self.pool).await?;
        let channels = sqlx::query_as::<_, AdminChannelRow>("SELECT id,channel_group_id,api_format::text AS api_format,name,base_url,enabled,auto_disabled,auto_disabled_reason,weight,proxy_id,config_template_id,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,(upstream_api_key IS NOT NULL) AS upstream_credential_configured,available_models,created_at,updated_at FROM channels ORDER BY id").fetch_all(&self.pool).await?;
        let model_rules = sqlx::query_as::<_, AdminModelRule>("SELECT r.id,r.client_model,r.api_format::text AS api_format,r.model_id,m.enabled AS model_enabled,r.upstream_model,r.description,r.channel_group_ids,r.channel_ids,r.enabled,r.updated_at FROM model_rules r JOIN models m ON m.id=r.model_id ORDER BY r.id").fetch_all(&self.pool).await?;
        Ok(AdminLists {
            api_keys,
            channel_groups,
            channels: channels.into_iter().map(Into::into).collect(),
            model_rules,
        })
    }

    pub async fn apply_admin_mutation(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        mutation: AdminMutation,
    ) -> Result<MutationResult, RepositoryError> {
        match mutation {
            AdminMutation::CreateApiKey(input) => {
                let id = Uuid::new_v4();
                // Two independent UUIDv4 values provide 32 random bytes in a transport-safe form.
                let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                let updated_at = sqlx::query_scalar("INSERT INTO api_keys (id, user_id, name, secret_value, status, expires_at, allowed_api_formats, permissions, allowed_group_ids) VALUES ($1,$2,$3,$4,'active',$5,$6::api_format[],$7,$8) RETURNING updated_at")
                    .bind(id).bind(input.user_id).bind(&input.name).bind(&secret).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).fetch_one(&mut **transaction).await?;
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
                let updated_at = sqlx::query_scalar("UPDATE api_keys SET name=$2,status=$3,expires_at=$4,allowed_api_formats=$5::api_format[],permissions=$6,allowed_group_ids=$7 WHERE id=$1 AND updated_at=$8 AND NOT (status='revoked' AND $3 <> 'revoked') RETURNING updated_at")
                    .bind(id).bind(&input.name).bind(&input.status).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).bind(expected_updated_at).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?;
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
        }
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
        "SELECT to_jsonb(api_keys) - 'secret_value' FROM api_keys WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(value) = value else {
        return Err(RepositoryError::NotFound);
    };
    Ok(value)
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
async fn channel_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: impl Into<ChannelMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if !is_empty_document(&input.override_document) || !is_empty_document(&input.health_check) {
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
