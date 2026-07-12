//! SQLx control-plane and append-only request-log repositories.

use std::fmt;

use chrono::{DateTime, Timelike, Utc};
use serde_json::Value;
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
#[derive(FromRow)]
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
    async fn load_transaction(
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
}
