use std::collections::{BTreeSet, HashSet};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction, postgres::PgConnection};
use uuid::Uuid;

use super::{ControlPlaneRepository, MutationResult, RepositoryError};

const CODEX_CONNECTOR_KIND: &str = "codex_oauth";
const CODEX_RESPONSES_API_FORMAT: &str = "open_ai_responses";
const QUOTA_WINDOW_IDENTITY_TOLERANCE: Duration = Duration::seconds(90);
const MANUAL_RESET_MATCH_WINDOW: Duration = Duration::minutes(15);

#[derive(Clone, Copy, FromRow)]
struct CodexPoolContext {
    connector_pool_id: Uuid,
    responses_channel_group_id: Uuid,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexOauthStartInput {
    pub label: String,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default = "default_quota_threshold_percent")]
    pub quota_threshold_percent: i16,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialImportInput {
    pub label: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default = "default_weight")]
    pub weight: i32,
    #[serde(default = "default_quota_threshold_percent")]
    pub quota_threshold_percent: i16,
    #[serde(default)]
    pub id_token: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialExportInput {
    #[serde(default)]
    pub credential_ids: Vec<Uuid>,
    #[serde(default = "default_include_proxies")]
    pub include_proxies: bool,
}

#[derive(Clone, Serialize)]
pub struct CodexCredentialExportBundle {
    #[serde(rename = "type")]
    pub export_type: &'static str,
    pub version: u8,
    pub exported_at: DateTime<Utc>,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub proxies: Vec<CodexCredentialExportProxy>,
    pub credentials: Vec<CodexCredentialExportItem>,
}

#[derive(Clone, Serialize, FromRow)]
pub struct CodexCredentialExportProxy {
    pub proxy_key: Uuid,
    pub name: String,
    pub proxy_url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub no_proxy_hosts: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Serialize)]
pub struct CodexCredentialExportItem {
    pub label: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub proxy_key: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialUpdateInput {
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialBatchInput {
    pub items: Vec<CodexCredentialBatchTarget>,
    pub operation: CodexCredentialBatchOperation,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialBatchTarget {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexCredentialBatchOperation {
    Enable,
    Disable,
    Delete,
}

#[derive(Clone)]
pub struct CodexCredentialCreate {
    pub channel_group_id: Uuid,
    pub label: String,
    pub enabled: bool,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub base_url: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub available_models: Vec<String>,
    pub quota: Option<CodexQuotaUpdate>,
}

#[derive(Clone, FromRow)]
pub struct CodexOauthFlowRecord {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub channel_group_id: Uuid,
    pub label: String,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub quota_threshold_percent: i16,
    pub redirect_uri: String,
    pub state_hash: Vec<u8>,
    pub code_verifier: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, FromRow)]
pub struct CodexCredentialRecord {
    pub channel_id: Uuid,
    pub channel_group_id: Uuid,
    pub connector_pool_id: Uuid,
    pub projection_channel_ids: Vec<Uuid>,
    pub label: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub last_refreshed_at: DateTime<Utc>,
    pub refresh_generation: i64,
    pub reauth_required: bool,
    pub quota_threshold_percent: i16,
    pub runtime_status: String,
    pub quota_allowed: Option<bool>,
    pub quota_limit_reached: Option<bool>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub quota_reset_credits_available: Option<i64>,
    pub quota_checked_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub last_error_summary: Option<String>,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub available_models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for CodexCredentialRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexCredentialRecord")
            .field("channel_id", &self.channel_id)
            .field("channel_group_id", &self.channel_group_id)
            .field("connector_pool_id", &self.connector_pool_id)
            .field("projection_channel_ids", &self.projection_channel_ids)
            .field("label", &self.label)
            .field("email", &self.email)
            .field("account_id", &self.account_id)
            .field("user_id", &self.user_id)
            .field("plan_type", &self.plan_type)
            .field("is_fedramp", &self.is_fedramp)
            .field("id_token", &"REDACTED")
            .field("access_token", &"REDACTED")
            .field("refresh_token", &"REDACTED")
            .field("access_token_expires_at", &self.access_token_expires_at)
            .field("last_refreshed_at", &self.last_refreshed_at)
            .field("refresh_generation", &self.refresh_generation)
            .field("reauth_required", &self.reauth_required)
            .field("quota_threshold_percent", &self.quota_threshold_percent)
            .field("runtime_status", &self.runtime_status)
            .field("quota_allowed", &self.quota_allowed)
            .field("quota_limit_reached", &self.quota_limit_reached)
            .field("primary_used_percent", &self.primary_used_percent)
            .field("secondary_used_percent", &self.secondary_used_percent)
            .field(
                "quota_reset_credits_available",
                &self.quota_reset_credits_available,
            )
            .field("quota_checked_at", &self.quota_checked_at)
            .field("last_error_code", &self.last_error_code)
            .field("last_error_summary", &self.last_error_summary)
            .field("proxy_id", &self.proxy_id)
            .field("weight", &self.weight)
            .field("enabled", &self.enabled)
            .field("available_models", &self.available_models)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct CodexCredentialView {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub label: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: bool,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub last_refreshed_at: DateTime<Utc>,
    pub quota_threshold_percent: i16,
    pub runtime_status: String,
    pub quota_allowed: Option<bool>,
    pub quota_limit_reached: Option<bool>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub quota_reset_credits_available: Option<i64>,
    pub quota_checked_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub last_error_summary: Option<String>,
    pub proxy_id: Option<Uuid>,
    pub weight: i32,
    pub enabled: bool,
    pub available_models: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct CodexQuotaUpdate {
    pub allowed: bool,
    pub limit_reached: bool,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub reset_credits_available: Option<i64>,
    pub checked_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct CodexQuotaWindowPeriodView {
    pub id: Uuid,
    pub credential_id: Uuid,
    pub window_kind: String,
    pub window_seconds: i32,
    pub started_at: DateTime<Utc>,
    pub scheduled_reset_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub reset_reason: Option<String>,
    pub initial_used_percent: i32,
    pub last_used_percent: i32,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodexQuotaWindowHistory {
    pub credential_id: Uuid,
    pub periods: Vec<CodexQuotaWindowPeriodView>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct SelfCodexQuotaCredentialView {
    pub id: Uuid,
    pub name: String,
    pub channel_group_id: Uuid,
    pub plan_type: Option<String>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub quota_checked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct SelfCodexQuotaWindowPeriodView {
    pub window_kind: String,
    pub window_seconds: i32,
    pub started_at: DateTime<Utc>,
    pub scheduled_reset_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub reset_reason: Option<String>,
    pub initial_used_percent: i32,
    pub last_used_percent: i32,
    pub first_observed_at: DateTime<Utc>,
    pub last_observed_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfCodexQuotaWindowHistory {
    pub credential_id: Uuid,
    pub name: String,
    pub channel_group_id: Uuid,
    pub plan_type: Option<String>,
    pub periods: Vec<SelfCodexQuotaWindowPeriodView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexQuotaResetOutcome {
    Reset,
    NothingToReset,
    NoCredit,
    AlreadyRedeemed,
}

impl CodexQuotaResetOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::NothingToReset => "nothing_to_reset",
            Self::NoCredit => "no_credit",
            Self::AlreadyRedeemed => "already_redeemed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CodexQuotaWindowKind {
    Primary,
    Secondary,
}

impl CodexQuotaWindowKind {
    const ALL: [Self; 2] = [Self::Primary, Self::Secondary];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

#[derive(Clone, FromRow)]
struct CurrentCodexQuotaWindowPeriod {
    id: Uuid,
    window_seconds: i32,
    started_at: DateTime<Utc>,
    scheduled_reset_at: DateTime<Utc>,
    initial_used_percent: i32,
    last_used_percent: i32,
    last_observed_at: DateTime<Utc>,
}

#[derive(Clone, Copy)]
struct ObservedCodexQuotaWindow {
    used_percent: i32,
    window_seconds: i32,
    reset_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct CodexTokenRefreshUpdate {
    pub expected_generation: i64,
    pub id_token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan_type: Option<String>,
    pub is_fedramp: Option<bool>,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub refreshed_at: DateTime<Utc>,
}

impl ControlPlaneRepository {
    pub async fn begin_codex_refresh(&self) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        self.pool.begin().await.map_err(RepositoryError::from)
    }

    pub async fn begin_codex_quota_reset(
        &self,
    ) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        self.pool.begin().await.map_err(RepositoryError::from)
    }

    pub async fn codex_credentials(
        &self,
        channel_group_id: Uuid,
    ) -> Result<Vec<CodexCredentialView>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialView>(
            "SELECT c.channel_id AS id,c.channel_group_id,c.label,c.email,c.account_id,c.user_id,c.plan_type, \
                    c.is_fedramp,c.access_token_expires_at,c.last_refreshed_at, \
                    c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                    c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                    c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                    c.secondary_reset_at,c.quota_reset_credits_available, \
                    c.quota_checked_at,c.last_error_code, \
                    c.last_error_summary,ch.proxy_id,ch.weight,c.enabled,ch.available_models, \
                    c.created_at,c.updated_at \
             FROM codex_oauth_credentials c \
             JOIN channels ch ON ch.id=c.channel_id \
             WHERE c.connector_pool_id=( \
                       SELECT connector_pool_id FROM channel_groups \
                       WHERE id=$1 AND connector_kind=$2 \
                   ) \
               AND c.deleted_at IS NULL \
             ORDER BY c.label,c.channel_id",
        )
        .bind(channel_group_id)
        .bind(CODEX_CONNECTOR_KIND)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_credential_view(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialView>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialView>(
            "SELECT c.channel_id AS id,c.channel_group_id,c.label,c.email,c.account_id,c.user_id,c.plan_type, \
                    c.is_fedramp,c.access_token_expires_at,c.last_refreshed_at, \
                    c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                    c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                    c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                    c.secondary_reset_at,c.quota_reset_credits_available, \
                    c.quota_checked_at,c.last_error_code, \
                    c.last_error_summary,ch.proxy_id,ch.weight,c.enabled,ch.available_models, \
                    c.created_at,c.updated_at \
             FROM codex_oauth_credentials c \
             JOIN channels ch ON ch.id=c.channel_id \
             WHERE c.channel_id=$1 AND c.deleted_at IS NULL",
        )
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_quota_window_history(
        &self,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<CodexQuotaWindowHistory, RepositoryError> {
        if !(1..=500).contains(&limit_per_window) {
            return Err(RepositoryError::Validation);
        }
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS( \
                 SELECT 1 FROM codex_oauth_credentials \
                 WHERE channel_id=$1 AND deleted_at IS NULL \
             )",
        )
        .bind(channel_id)
        .fetch_one(&self.pool)
        .await?;
        if !exists {
            return Err(RepositoryError::NotFound);
        }
        let periods = sqlx::query_as::<_, CodexQuotaWindowPeriodView>(
            "WITH ranked AS ( \
                 SELECT id,credential_id,window_kind,window_seconds,started_at, \
                        scheduled_reset_at,ended_at,reset_reason,initial_used_percent, \
                        last_used_percent,first_observed_at,last_observed_at, \
                        row_number() OVER ( \
                            PARTITION BY window_kind \
                            ORDER BY started_at DESC,id DESC \
                        ) AS window_rank \
                 FROM codex_quota_window_periods \
                 WHERE credential_id=$1 \
                   AND ( \
                       reset_reason IS DISTINCT FROM 'openai_official' \
                       OR initial_used_percent<>0 \
                       OR last_used_percent<>0 \
                   ) \
             ) \
             SELECT id,credential_id,window_kind,window_seconds,started_at, \
                    scheduled_reset_at,ended_at,reset_reason,initial_used_percent, \
                    last_used_percent,first_observed_at,last_observed_at \
             FROM ranked \
             WHERE window_rank <= $2 \
             ORDER BY window_kind,started_at DESC,id DESC",
        )
        .bind(channel_id)
        .bind(limit_per_window)
        .fetch_all(&self.pool)
        .await?;
        Ok(CodexQuotaWindowHistory {
            credential_id: channel_id,
            periods,
        })
    }

    pub async fn self_codex_quota_credentials(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<SelfCodexQuotaCredentialView>, RepositoryError> {
        sqlx::query_as::<_, SelfCodexQuotaCredentialView>(
            "SELECT credential.channel_id AS id, \
                    credential.channel_id::text AS name, \
                    visibility.channel_group_id,credential.plan_type, \
                    credential.primary_used_percent,credential.primary_window_seconds, \
                    credential.primary_reset_at,credential.secondary_used_percent, \
                    credential.secondary_window_seconds,credential.secondary_reset_at, \
                    credential.quota_checked_at \
             FROM users AS console_user \
             JOIN user_group_codex_quota_visibility AS visibility \
               ON visibility.user_group_id=console_user.user_group_id \
             JOIN channel_groups AS visible_group \
               ON visible_group.id=visibility.channel_group_id \
              AND visible_group.connector_kind=$2 \
              AND visible_group.api_format=$3::api_format \
             JOIN codex_oauth_credentials AS credential \
               ON credential.connector_pool_id=visible_group.connector_pool_id \
             WHERE console_user.id=$1 \
               AND console_user.status='active' \
               AND console_user.deleted_at IS NULL \
               AND credential.deleted_at IS NULL \
             ORDER BY visibility.channel_group_id,credential.channel_id",
        )
        .bind(user_id)
        .bind(CODEX_CONNECTOR_KIND)
        .bind(CODEX_RESPONSES_API_FORMAT)
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn self_codex_quota_window_history(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<SelfCodexQuotaWindowHistory, RepositoryError> {
        if !(1..=500).contains(&limit_per_window) {
            return Err(RepositoryError::Validation);
        }
        let credential = sqlx::query_as::<_, SelfCodexQuotaCredentialView>(
            "SELECT credential.channel_id AS id, \
                    credential.channel_id::text AS name, \
                    visibility.channel_group_id,credential.plan_type, \
                    credential.primary_used_percent,credential.primary_window_seconds, \
                    credential.primary_reset_at,credential.secondary_used_percent, \
                    credential.secondary_window_seconds,credential.secondary_reset_at, \
                    credential.quota_checked_at \
             FROM users AS console_user \
             JOIN user_group_codex_quota_visibility AS visibility \
               ON visibility.user_group_id=console_user.user_group_id \
             JOIN channel_groups AS visible_group \
               ON visible_group.id=visibility.channel_group_id \
              AND visible_group.connector_kind=$3 \
              AND visible_group.api_format=$4::api_format \
             JOIN codex_oauth_credentials AS credential \
               ON credential.connector_pool_id=visible_group.connector_pool_id \
             WHERE console_user.id=$1 \
               AND console_user.status='active' \
               AND console_user.deleted_at IS NULL \
               AND credential.channel_id=$2 \
               AND credential.deleted_at IS NULL",
        )
        .bind(user_id)
        .bind(channel_id)
        .bind(CODEX_CONNECTOR_KIND)
        .bind(CODEX_RESPONSES_API_FORMAT)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        let periods = sqlx::query_as::<_, SelfCodexQuotaWindowPeriodView>(
            "WITH ranked AS ( \
                 SELECT period.window_kind,period.window_seconds,period.started_at, \
                        period.scheduled_reset_at,period.ended_at,period.reset_reason, \
                        period.initial_used_percent,period.last_used_percent, \
                        period.first_observed_at,period.last_observed_at, \
                        row_number() OVER ( \
                            PARTITION BY period.window_kind \
                            ORDER BY period.started_at DESC,period.id DESC \
                        ) AS window_rank \
                 FROM codex_quota_window_periods AS period \
                 JOIN codex_oauth_credentials AS credential \
                   ON credential.channel_id=period.credential_id \
                  AND credential.deleted_at IS NULL \
                 JOIN channel_groups AS visible_group \
                   ON visible_group.connector_pool_id=credential.connector_pool_id \
                  AND visible_group.connector_kind=$4 \
                  AND visible_group.api_format=$5::api_format \
                 JOIN user_group_codex_quota_visibility AS visibility \
                   ON visibility.channel_group_id=visible_group.id \
                 JOIN users AS console_user \
                   ON console_user.user_group_id=visibility.user_group_id \
                  AND console_user.status='active' \
                  AND console_user.deleted_at IS NULL \
                 WHERE console_user.id=$1 \
                   AND credential.channel_id=$2 \
                   AND ( \
                       period.reset_reason IS DISTINCT FROM 'openai_official' \
                       OR period.initial_used_percent<>0 \
                       OR period.last_used_percent<>0 \
                   ) \
             ) \
             SELECT window_kind,window_seconds,started_at,scheduled_reset_at, \
                    ended_at,reset_reason,initial_used_percent,last_used_percent, \
                    first_observed_at,last_observed_at \
             FROM ranked \
             WHERE window_rank <= $3 \
             ORDER BY window_kind,started_at DESC",
        )
        .bind(user_id)
        .bind(channel_id)
        .bind(limit_per_window)
        .bind(CODEX_CONNECTOR_KIND)
        .bind(CODEX_RESPONSES_API_FORMAT)
        .fetch_all(&self.pool)
        .await?;
        Ok(SelfCodexQuotaWindowHistory {
            credential_id: credential.id,
            name: credential.name,
            channel_group_id: credential.channel_group_id,
            plan_type: credential.plan_type,
            periods,
        })
    }

    pub async fn codex_credential(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
            "WHERE c.channel_id=$1 AND c.deleted_at IS NULL",
        ))
        .bind(channel_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_credential_for_update(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
            "WHERE c.channel_id=$1 AND c.deleted_at IS NULL FOR UPDATE OF c,ch",
        ))
        .bind(channel_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn load_codex_credentials(
        &self,
    ) -> Result<Vec<CodexCredentialRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
            "WHERE c.deleted_at IS NULL ORDER BY c.channel_id",
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn set_codex_user_id_if_missing(
        &self,
        channel_id: Uuid,
        user_id: &str,
    ) -> Result<bool, RepositoryError> {
        if user_id.trim().is_empty() || user_id.len() > 300 {
            return Err(RepositoryError::Validation);
        }
        let updated = sqlx::query(
            "UPDATE codex_oauth_credentials AS target \
             SET user_id=$2 \
             WHERE target.channel_id=$1 AND target.user_id IS NULL \
               AND target.deleted_at IS NULL \
               AND NOT EXISTS( \
                   SELECT 1 FROM codex_oauth_credentials AS existing \
                   WHERE existing.connector_pool_id=target.connector_pool_id \
                     AND existing.account_id IS NOT DISTINCT FROM target.account_id \
                     AND existing.user_id=$2 \
                     AND existing.deleted_at IS NULL \
               )",
        )
        .bind(channel_id)
        .bind(user_id.trim())
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn export_codex_credentials(
        &self,
        channel_group_id: Uuid,
        input: CodexCredentialExportInput,
    ) -> Result<CodexCredentialExportBundle, RepositoryError> {
        const MAX_SELECTED_CREDENTIALS: usize = 1_000;

        if input.credential_ids.len() > MAX_SELECTED_CREDENTIALS {
            return Err(RepositoryError::Validation);
        }
        let selected_ids = input
            .credential_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if selected_ids.len() != input.credential_ids.len() {
            return Err(RepositoryError::Validation);
        }

        let pool = codex_pool_context_pool(&self.pool, channel_group_id).await?;
        let channel_group_name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM channel_groups \
             WHERE id=$1 AND connector_kind=$2",
        )
        .bind(channel_group_id)
        .bind(CODEX_CONNECTOR_KIND)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::NotFound)?;

        let records = if selected_ids.is_empty() {
            sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
                "WHERE c.connector_pool_id=$1 AND c.deleted_at IS NULL \
                 ORDER BY c.label,c.channel_id",
            ))
            .bind(pool.connector_pool_id)
            .fetch_all(&self.pool)
            .await?
        } else {
            let selected_ids = selected_ids.into_iter().collect::<Vec<_>>();
            let records = sqlx::query_as::<_, CodexCredentialRecord>(&credential_select(
                "WHERE c.connector_pool_id=$1 AND c.channel_id=ANY($2) \
                 AND c.deleted_at IS NULL \
                 ORDER BY c.label,c.channel_id",
            ))
            .bind(pool.connector_pool_id)
            .bind(&selected_ids)
            .fetch_all(&self.pool)
            .await?;
            if records.len() != selected_ids.len() {
                return Err(RepositoryError::NotFound);
            }
            records
        };

        let proxy_ids = records
            .iter()
            .filter_map(|record| record.proxy_id)
            .collect::<BTreeSet<_>>();
        let proxies = if input.include_proxies && !proxy_ids.is_empty() {
            let proxy_ids = proxy_ids.into_iter().collect::<Vec<_>>();
            sqlx::query_as::<_, CodexCredentialExportProxy>(
                "SELECT id AS proxy_key,name,proxy_url,username,password,no_proxy_hosts,enabled \
                 FROM proxies WHERE id=ANY($1) ORDER BY name,id",
            )
            .bind(&proxy_ids)
            .fetch_all(&self.pool)
            .await?
        } else {
            Vec::new()
        };
        let credentials = records
            .into_iter()
            .map(|record| CodexCredentialExportItem {
                label: record.label,
                email: record.email,
                account_id: record.account_id,
                user_id: record.user_id,
                plan_type: record.plan_type,
                is_fedramp: record.is_fedramp,
                id_token: record.id_token,
                access_token: record.access_token,
                refresh_token: record.refresh_token,
                proxy_key: record.proxy_id.filter(|_| input.include_proxies),
                weight: record.weight,
                quota_threshold_percent: record.quota_threshold_percent,
                enabled: record.enabled,
            })
            .collect();

        Ok(CodexCredentialExportBundle {
            export_type: "ai-gateway-codex-credentials",
            version: 1,
            exported_at: Utc::now(),
            channel_group_id,
            channel_group_name,
            proxies,
            credentials,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_codex_oauth_flow(
        &self,
        actor_user_id: Uuid,
        channel_group_id: Uuid,
        input: CodexOauthStartInput,
        redirect_uri: String,
        state_hash: Vec<u8>,
        code_verifier: String,
        expires_at: DateTime<Utc>,
    ) -> Result<CodexOauthFlowRecord, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        let _ = validate_codex_group_and_proxy_pool(&self.pool, channel_group_id, input.proxy_id)
            .await?;
        let id = Uuid::new_v4();
        sqlx::query_as::<_, CodexOauthFlowRecord>(
            "INSERT INTO codex_oauth_flows \
             (id,actor_user_id,channel_group_id,label,proxy_id,weight,quota_threshold_percent, \
              redirect_uri,state_hash,code_verifier,expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) \
             RETURNING id,actor_user_id,channel_group_id,label,proxy_id,weight, \
                       quota_threshold_percent,redirect_uri,state_hash,code_verifier,expires_at",
        )
        .bind(id)
        .bind(actor_user_id)
        .bind(channel_group_id)
        .bind(input.label.trim())
        .bind(input.proxy_id)
        .bind(input.weight)
        .bind(input.quota_threshold_percent)
        .bind(redirect_uri)
        .bind(state_hash)
        .bind(code_verifier)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn codex_oauth_flow(
        &self,
        id: Uuid,
        actor_user_id: Uuid,
    ) -> Result<Option<CodexOauthFlowRecord>, RepositoryError> {
        sqlx::query_as::<_, CodexOauthFlowRecord>(
            "SELECT id,actor_user_id,channel_group_id,label,proxy_id,weight, \
                    quota_threshold_percent,redirect_uri,state_hash,code_verifier,expires_at \
             FROM codex_oauth_flows \
             WHERE id=$1 AND actor_user_id=$2 AND completed_at IS NULL AND expires_at>now()",
        )
        .bind(id)
        .bind(actor_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn insert_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        let requested_channel_group_id = input.channel_group_id;
        let pool = validate_codex_group_and_proxy_transaction(
            transaction,
            requested_channel_group_id,
            input.proxy_id,
        )
        .await?;
        let input = CodexCredentialCreate {
            channel_group_id: pool.responses_channel_group_id,
            ..input
        };
        if input
            .account_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
            || input
                .user_id
                .as_ref()
                .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
            || (input.account_id.is_none() && input.user_id.is_none())
            || input.id_token.is_empty()
            || input.access_token.is_empty()
            || input.refresh_token.is_empty()
            || input.available_models.is_empty()
        {
            return Err(RepositoryError::Validation);
        }
        let existing_channel_id = existing_codex_channel_id(
            transaction,
            pool.connector_pool_id,
            input.account_id.as_deref(),
            input.user_id.as_deref(),
            input.email.as_deref(),
        )
        .await?;
        if let Some(flow_id) = oauth_flow_id {
            let updated = sqlx::query(
                "UPDATE codex_oauth_flows SET completed_at=now() \
                 WHERE id=$1 AND channel_group_id=$2 AND completed_at IS NULL AND expires_at>now()",
            )
            .bind(flow_id)
            .bind(requested_channel_group_id)
            .execute(&mut **transaction)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(RepositoryError::Conflict);
            }
        }

        if let Some(channel_id) = existing_channel_id {
            let before = codex_credential_audit(transaction, channel_id).await?;
            let existing_quota_checked_at = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
                "SELECT quota_checked_at FROM codex_oauth_credentials WHERE channel_id=$1",
            )
            .bind(channel_id)
            .fetch_one(&mut **transaction)
            .await?;
            sqlx::query(
                "UPDATE channels SET \
                 name=$2,base_url=$3,enabled=true,weight=$4,proxy_id=$5,available_models=$6, \
                 supports_websocket=true,supports_standalone_web_search=true \
                 WHERE id=$1",
            )
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(&input.base_url)
            .bind(input.weight)
            .bind(input.proxy_id)
            .bind(&input.available_models)
            .execute(&mut **transaction)
            .await?;

            let quota = input.quota.as_ref().filter(|quota| {
                existing_quota_checked_at.is_none_or(|checked_at| quota.checked_at >= checked_at)
            });
            let has_quota = quota.is_some();
            let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
                "UPDATE codex_oauth_credentials SET \
                 label=$2,email=$3,plan_type=$4,is_fedramp=$5,id_token=$6,access_token=$7, \
                 refresh_token=$8,access_token_expires_at=$9,last_refreshed_at=$10, \
                 refresh_generation=refresh_generation+1,reauth_required=false,enabled=$12, \
                 quota_threshold_percent=$11,runtime_status=CASE \
                     WHEN NOT $12 THEN 'disabled' \
                     WHEN $13 THEN CASE \
                         WHEN NOT $14 OR $15 THEN 'unavailable' \
                         WHEN GREATEST(COALESCE($16,0),COALESCE($19,0)) >= $11 \
                             THEN 'draining' \
                         ELSE 'active' END \
                     WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                     WHEN GREATEST(COALESCE(primary_used_percent,0), \
                                   COALESCE(secondary_used_percent,0)) >= $11 \
                         THEN 'draining' \
                     ELSE 'active' END, \
                 quota_allowed=CASE WHEN $13 THEN $14 ELSE quota_allowed END, \
                 quota_limit_reached=CASE WHEN $13 THEN $15 ELSE quota_limit_reached END, \
                 primary_used_percent=CASE WHEN $13 THEN $16 ELSE primary_used_percent END, \
                 primary_window_seconds=CASE WHEN $13 THEN $17 ELSE primary_window_seconds END, \
                 primary_reset_at=CASE WHEN $13 THEN $18 ELSE primary_reset_at END, \
                 secondary_used_percent=CASE WHEN $13 THEN $19 ELSE secondary_used_percent END, \
                 secondary_window_seconds=CASE WHEN $13 THEN $20 ELSE secondary_window_seconds END, \
                 secondary_reset_at=CASE WHEN $13 THEN $21 ELSE secondary_reset_at END, \
                 quota_checked_at=CASE WHEN $13 THEN $22 ELSE quota_checked_at END, \
                 quota_reset_credits_available=CASE \
                     WHEN $13 THEN $23 ELSE quota_reset_credits_available END, \
                 last_error_code=NULL,last_error_summary=NULL, \
                 user_id=COALESCE($24,user_id) \
                 WHERE channel_id=$1 AND deleted_at IS NULL \
                 RETURNING updated_at",
            )
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(input.email)
            .bind(input.plan_type)
            .bind(input.is_fedramp)
            .bind(input.id_token)
            .bind(input.access_token)
            .bind(input.refresh_token)
            .bind(input.access_token_expires_at)
            .bind(Utc::now())
            .bind(input.quota_threshold_percent)
            .bind(input.enabled)
            .bind(has_quota)
            .bind(quota.map(|quota| quota.allowed))
            .bind(quota.map(|quota| quota.limit_reached))
            .bind(quota.and_then(|quota| quota.primary_used_percent))
            .bind(quota.and_then(|quota| quota.primary_window_seconds))
            .bind(quota.and_then(|quota| quota.primary_reset_at))
            .bind(quota.and_then(|quota| quota.secondary_used_percent))
            .bind(quota.and_then(|quota| quota.secondary_window_seconds))
            .bind(quota.and_then(|quota| quota.secondary_reset_at))
            .bind(quota.map(|quota| quota.checked_at))
            .bind(quota.and_then(|quota| quota.reset_credits_available))
            .bind(input.user_id)
            .fetch_one(&mut **transaction)
            .await?;
            if let Some(quota) = quota {
                reconcile_codex_quota_windows(transaction, channel_id, quota).await?;
            }

            return Ok(MutationResult {
                id: channel_id,
                object_type: "codex_oauth_credential",
                action: "update",
                before_redacted: before,
                after_redacted: codex_credential_audit(transaction, channel_id).await?,
                created_secret: None,
                reason: None,
                updated_at,
                correlation_id: None,
            });
        }

        let channel_id = Uuid::new_v4();
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "INSERT INTO channels \
             (id,channel_group_id,api_format,name,base_url,enabled,weight,billing_multiplier, \
              proxy_id,override_document,upstream_auth_kind,available_models, \
              auto_disable_allowed,supports_websocket,supports_standalone_web_search) \
             VALUES ($1,$2,$3::api_format,$4,$5,true,$6,1,$7,'{}','none',$8,false,true,true) \
             RETURNING updated_at",
        )
        .bind(channel_id)
        .bind(input.channel_group_id)
        .bind(CODEX_RESPONSES_API_FORMAT)
        .bind(input.label.trim())
        .bind(input.base_url)
        .bind(input.weight)
        .bind(input.proxy_id)
        .bind(&input.available_models)
        .fetch_one(&mut **transaction)
        .await?;

        let quota = input.quota.as_ref();
        let runtime_status = if input.enabled {
            quota.map_or("active", |quota| {
                runtime_status_for_quota(quota, input.quota_threshold_percent)
            })
        } else {
            "disabled"
        };
        sqlx::query(
            "INSERT INTO codex_oauth_credentials \
             (channel_id,channel_group_id,connector_pool_id,label,email,account_id,user_id,plan_type,is_fedramp,id_token, \
              access_token,refresh_token,access_token_expires_at,last_refreshed_at, \
              enabled,quota_threshold_percent,runtime_status,quota_allowed,quota_limit_reached, \
              primary_used_percent,primary_window_seconds,primary_reset_at, \
              secondary_used_percent,secondary_window_seconds,secondary_reset_at, \
              quota_reset_credits_available,quota_checked_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27)",
        )
        .bind(channel_id)
        .bind(input.channel_group_id)
        .bind(pool.connector_pool_id)
        .bind(input.label.trim())
        .bind(input.email)
        .bind(input.account_id)
        .bind(input.user_id)
        .bind(input.plan_type)
        .bind(input.is_fedramp)
        .bind(input.id_token)
        .bind(input.access_token)
        .bind(input.refresh_token)
        .bind(input.access_token_expires_at)
        .bind(Utc::now())
        .bind(input.enabled)
        .bind(input.quota_threshold_percent)
        .bind(runtime_status)
        .bind(quota.map(|quota| quota.allowed))
        .bind(quota.map(|quota| quota.limit_reached))
        .bind(quota.and_then(|quota| quota.primary_used_percent))
        .bind(quota.and_then(|quota| quota.primary_window_seconds))
        .bind(quota.and_then(|quota| quota.primary_reset_at))
        .bind(quota.and_then(|quota| quota.secondary_used_percent))
        .bind(quota.and_then(|quota| quota.secondary_window_seconds))
        .bind(quota.and_then(|quota| quota.secondary_reset_at))
        .bind(quota.and_then(|quota| quota.reset_credits_available))
        .bind(quota.map(|quota| quota.checked_at))
        .execute(&mut **transaction)
        .await?;
        if let Some(quota) = quota {
            reconcile_codex_quota_windows(transaction, channel_id, quota).await?;
        }

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "create",
            before_redacted: json!({}),
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn update_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        validate_credential_settings(&input.label, input.weight, input.quota_threshold_percent)?;
        let before = codex_credential_audit(transaction, channel_id).await?;
        let group_id = before["channel_group_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or(RepositoryError::Validation)?;
        validate_codex_group_and_proxy_transaction(transaction, group_id, input.proxy_id).await?;
        let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
            "UPDATE codex_oauth_credentials \
             SET label=$2,quota_threshold_percent=$3,enabled=$4,runtime_status=CASE \
                 WHEN NOT $4 THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN GREATEST(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= $3 \
                     THEN 'draining' \
                 ELSE 'active' END \
             WHERE channel_id=$1 AND updated_at=$5 AND deleted_at IS NULL \
             RETURNING updated_at",
        )
        .bind(channel_id)
        .bind(input.label.trim())
        .bind(input.quota_threshold_percent)
        .bind(input.enabled)
        .bind(expected_updated_at)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?;
        sqlx::query("UPDATE channels SET name=$2,proxy_id=$3,weight=$4 WHERE id=$1")
            .bind(channel_id)
            .bind(input.label.trim())
            .bind(input.proxy_id)
            .bind(input.weight)
            .execute(&mut **transaction)
            .await?;

        Ok(MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "update",
            before_redacted: before,
            after_redacted: codex_credential_audit(transaction, channel_id).await?,
            created_secret: None,
            reason: None,
            updated_at,
            correlation_id: None,
        })
    }

    pub async fn delete_codex_credential(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, RepositoryError> {
        delete_codex_credential(transaction, channel_id, None, expected_updated_at).await
    }

    pub async fn update_codex_credentials_batch(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        const MAX_BATCH_SIZE: usize = 100;

        if input.items.is_empty() || input.items.len() > MAX_BATCH_SIZE {
            return Err(RepositoryError::Validation);
        }
        let pool =
            validate_codex_group_and_proxy_transaction(transaction, channel_group_id, None).await?;
        let mut ids = HashSet::with_capacity(input.items.len());
        if input.items.iter().any(|item| !ids.insert(item.id)) {
            return Err(RepositoryError::Validation);
        }

        let mut results = Vec::with_capacity(input.items.len());
        for item in input.items {
            let result = match input.operation {
                CodexCredentialBatchOperation::Enable => {
                    set_codex_credential_enabled(
                        transaction,
                        item.id,
                        pool.connector_pool_id,
                        item.updated_at,
                        true,
                    )
                    .await?
                }
                CodexCredentialBatchOperation::Disable => {
                    set_codex_credential_enabled(
                        transaction,
                        item.id,
                        pool.connector_pool_id,
                        item.updated_at,
                        false,
                    )
                    .await?
                }
                CodexCredentialBatchOperation::Delete => {
                    delete_codex_credential(
                        transaction,
                        item.id,
                        Some(pool.connector_pool_id),
                        item.updated_at,
                    )
                    .await?
                }
            };
            results.push(result);
        }
        Ok(results)
    }

    pub async fn persist_codex_token_refresh_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        update: CodexTokenRefreshUpdate,
    ) -> Result<bool, RepositoryError> {
        let updated = sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             id_token=COALESCE($3,id_token),access_token=COALESCE($4,access_token), \
             refresh_token=COALESCE($5,refresh_token),email=COALESCE($6,email), \
             account_id=COALESCE($7,account_id),plan_type=COALESCE($8,plan_type), \
             is_fedramp=COALESCE($9,is_fedramp),access_token_expires_at=$10, \
             last_refreshed_at=$11,refresh_generation=refresh_generation+1, \
             user_id=COALESCE($12,user_id), \
             reauth_required=false, \
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
                 WHEN GREATEST(COALESCE(primary_used_percent,0), \
                               COALESCE(secondary_used_percent,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             last_error_code=NULL,last_error_summary=NULL \
             WHERE channel_id=$1 AND refresh_generation=$2 AND deleted_at IS NULL",
        )
        .bind(channel_id)
        .bind(update.expected_generation)
        .bind(update.id_token)
        .bind(update.access_token)
        .bind(update.refresh_token)
        .bind(update.email)
        .bind(update.account_id)
        .bind(update.plan_type)
        .bind(update.is_fedramp)
        .bind(update.access_token_expires_at)
        .bind(update.refreshed_at)
        .bind(update.user_id)
        .execute(&mut **transaction)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    pub async fn persist_codex_quota(
        &self,
        channel_id: Uuid,
        quota: CodexQuotaUpdate,
    ) -> Result<(), RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let locked = sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE channel_id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(channel_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if locked.is_none() {
            return Err(RepositoryError::NotFound);
        }
        reconcile_codex_quota_windows(&mut transaction, channel_id, &quota).await?;
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             runtime_status=CASE \
                 WHEN NOT enabled THEN 'disabled' \
                 WHEN reauth_required THEN 'unavailable' \
                 WHEN NOT $2 OR $3 THEN 'unavailable' \
                 WHEN GREATEST(COALESCE($4,0),COALESCE($7,0)) >= quota_threshold_percent \
                     THEN 'draining' \
                 ELSE 'active' END, \
             quota_allowed=$2,quota_limit_reached=$3, \
             primary_used_percent=$4,primary_window_seconds=$5,primary_reset_at=$6, \
             secondary_used_percent=$7,secondary_window_seconds=$8,secondary_reset_at=$9, \
             quota_checked_at=$10,quota_reset_credits_available=$11, \
             last_error_code=CASE WHEN reauth_required THEN last_error_code ELSE NULL END, \
             last_error_summary=CASE WHEN reauth_required THEN last_error_summary ELSE NULL END \
             WHERE channel_id=$1 AND deleted_at IS NULL \
               AND (quota_checked_at IS NULL OR quota_checked_at <= $10)",
        )
        .bind(channel_id)
        .bind(quota.allowed)
        .bind(quota.limit_reached)
        .bind(quota.primary_used_percent)
        .bind(quota.primary_window_seconds)
        .bind(quota.primary_reset_at)
        .bind(quota.secondary_used_percent)
        .bind(quota.secondary_window_seconds)
        .bind(quota.secondary_reset_at)
        .bind(quota.checked_at)
        .bind(quota.reset_credits_available)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_codex_quota_reset(
        &self,
        actor_user_id: Uuid,
        channel_id: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
    ) -> Result<Uuid, RepositoryError> {
        if !(0..=2).contains(&windows_reset) {
            return Err(RepositoryError::Validation);
        }
        let mut transaction = self.begin_serializable().await?;
        let reset_credits_available = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT quota_reset_credits_available \
             FROM codex_oauth_credentials \
             WHERE channel_id=$1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(channel_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        let correlation_id = self
            .record_codex_quota_reset_transaction(
                &mut transaction,
                actor_user_id,
                channel_id,
                event_id,
                requested_at,
                outcome,
                windows_reset,
                reset_credits_available,
            )
            .await?;
        transaction.commit().await?;
        Ok(correlation_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_codex_quota_reset_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor_user_id: Uuid,
        channel_id: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
        reset_credits_available: Option<i64>,
    ) -> Result<Uuid, RepositoryError> {
        if !(0..=2).contains(&windows_reset) {
            return Err(RepositoryError::Validation);
        }
        let correlation_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO codex_quota_reset_events \
             (id,credential_id,actor_user_id,requested_at,outcome,windows_reset,correlation_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(event_id)
        .bind(channel_id)
        .bind(actor_user_id)
        .bind(requested_at)
        .bind(outcome.as_str())
        .bind(windows_reset)
        .bind(correlation_id)
        .execute(&mut **transaction)
        .await?;
        let mutation = MutationResult {
            id: channel_id,
            object_type: "codex_oauth_credential",
            action: "reset_quota",
            before_redacted: json!({
                "quota_reset_credits_available": reset_credits_available,
            }),
            after_redacted: json!({
                "outcome": outcome.as_str(),
                "windows_reset": windows_reset,
                "redeem_request_id": event_id,
            }),
            created_secret: None,
            reason: Some("manual_reset_credit".into()),
            updated_at: requested_at,
            correlation_id: Some(correlation_id),
        };
        self.insert_audit(transaction, actor_user_id, &mutation, correlation_id)
            .await?;
        Ok(correlation_id)
    }

    pub async fn mark_codex_credential_error(
        &self,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             reauth_required=reauth_required OR $2, \
             runtime_status=CASE WHEN $2 THEN 'unavailable' ELSE runtime_status END, \
             last_error_code=CASE WHEN reauth_required AND NOT $2 THEN last_error_code ELSE $3 END, \
             last_error_summary=CASE WHEN reauth_required AND NOT $2 THEN last_error_summary ELSE $4 END \
             WHERE channel_id=$1 AND deleted_at IS NULL",
        )
        .bind(channel_id)
        .bind(permanent)
        .bind(code)
        .bind(summary)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_codex_credential_error_transaction(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE codex_oauth_credentials SET \
             reauth_required=reauth_required OR $2, \
             runtime_status=CASE WHEN $2 THEN 'unavailable' ELSE runtime_status END, \
             last_error_code=CASE WHEN reauth_required AND NOT $2 THEN last_error_code ELSE $3 END, \
             last_error_summary=CASE WHEN reauth_required AND NOT $2 THEN last_error_summary ELSE $4 END \
             WHERE channel_id=$1 AND deleted_at IS NULL",
        )
        .bind(channel_id)
        .bind(permanent)
        .bind(code)
        .bind(summary)
        .execute(&mut **transaction)
        .await?;
        Ok(())
    }

    pub async fn cleanup_codex_oauth_flows(&self) -> Result<u64, RepositoryError> {
        let deleted = sqlx::query(
            "DELETE FROM codex_oauth_flows \
             WHERE expires_at < now() OR completed_at IS NOT NULL",
        )
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected())
    }
}

async fn reconcile_codex_quota_windows(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    quota: &CodexQuotaUpdate,
) -> Result<(), RepositoryError> {
    for kind in CodexQuotaWindowKind::ALL {
        let Some(observation) = observed_codex_quota_window(quota, kind)? else {
            continue;
        };
        reconcile_codex_quota_window(transaction, channel_id, kind, observation, quota.checked_at)
            .await?;
    }
    Ok(())
}

fn observed_codex_quota_window(
    quota: &CodexQuotaUpdate,
    kind: CodexQuotaWindowKind,
) -> Result<Option<ObservedCodexQuotaWindow>, RepositoryError> {
    let values = match kind {
        CodexQuotaWindowKind::Primary => (
            quota.primary_used_percent,
            quota.primary_window_seconds,
            quota.primary_reset_at,
        ),
        CodexQuotaWindowKind::Secondary => (
            quota.secondary_used_percent,
            quota.secondary_window_seconds,
            quota.secondary_reset_at,
        ),
    };
    match values {
        (None, None, None) => Ok(None),
        (Some(used_percent), Some(window_seconds), Some(reset_at))
            if (0..=100).contains(&used_percent) && window_seconds > 0 =>
        {
            Ok(Some(ObservedCodexQuotaWindow {
                used_percent,
                window_seconds,
                reset_at,
            }))
        }
        _ => Err(RepositoryError::Validation),
    }
}

async fn reconcile_codex_quota_window(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    observation: ObservedCodexQuotaWindow,
    checked_at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let started_at = observation
        .reset_at
        .checked_sub_signed(Duration::seconds(i64::from(observation.window_seconds)))
        .ok_or(RepositoryError::Validation)?;
    let current = sqlx::query_as::<_, CurrentCodexQuotaWindowPeriod>(
        "SELECT id,window_seconds,started_at,scheduled_reset_at, \
                initial_used_percent,last_used_percent,last_observed_at \
         FROM codex_quota_window_periods \
         WHERE credential_id=$1 AND window_kind=$2 AND ended_at IS NULL \
         FOR UPDATE",
    )
    .bind(channel_id)
    .bind(kind.as_str())
    .fetch_optional(&mut **transaction)
    .await?;

    let Some(current) = current else {
        insert_codex_quota_window_period(
            transaction,
            channel_id,
            kind,
            observation,
            started_at,
            checked_at,
        )
        .await?;
        return Ok(());
    };

    if checked_at < current.last_observed_at {
        return Ok(());
    }

    let same_period = current.window_seconds == observation.window_seconds
        && timestamps_within(
            current.scheduled_reset_at,
            observation.reset_at,
            QUOTA_WINDOW_IDENTITY_TOLERANCE,
        );
    if same_period
        || started_at
            <= current
                .started_at
                .checked_add_signed(QUOTA_WINDOW_IDENTITY_TOLERANCE)
                .unwrap_or(current.started_at)
    {
        sqlx::query(
            "UPDATE codex_quota_window_periods \
             SET last_used_percent=CASE \
                     WHEN last_observed_at <= $3 THEN $2 \
                     ELSE last_used_percent \
                 END, \
                 last_observed_at=GREATEST(last_observed_at,$3) \
             WHERE id=$1",
        )
        .bind(current.id)
        .bind(observation.used_percent)
        .bind(checked_at)
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    let natural_boundary = current
        .scheduled_reset_at
        .checked_sub_signed(QUOTA_WINDOW_IDENTITY_TOLERANCE)
        .unwrap_or(current.scheduled_reset_at);
    let manual_reset = if started_at < natural_boundary {
        claim_manual_codex_quota_reset(transaction, channel_id, kind, started_at, checked_at)
            .await?
    } else {
        false
    };

    if current.initial_used_percent == 0
        && current.last_used_percent == 0
        && started_at < natural_boundary
        && !manual_reset
    {
        sqlx::query(
            "UPDATE codex_quota_window_periods \
             SET window_seconds=$2,started_at=$3,scheduled_reset_at=$4, \
                 last_used_percent=$5,last_observed_at=GREATEST(last_observed_at,$6) \
             WHERE id=$1",
        )
        .bind(current.id)
        .bind(observation.window_seconds)
        .bind(started_at)
        .bind(observation.reset_at)
        .bind(observation.used_percent)
        .bind(checked_at)
        .execute(&mut **transaction)
        .await?;
        return Ok(());
    }

    let (reset_reason, ended_at) = if started_at >= natural_boundary {
        ("natural", current.scheduled_reset_at)
    } else if manual_reset {
        ("manual", started_at)
    } else {
        ("openai_official", started_at)
    };
    sqlx::query(
        "UPDATE codex_quota_window_periods \
         SET ended_at=$2,reset_reason=$3 \
         WHERE id=$1",
    )
    .bind(current.id)
    .bind(ended_at)
    .bind(reset_reason)
    .execute(&mut **transaction)
    .await?;
    insert_codex_quota_window_period(
        transaction,
        channel_id,
        kind,
        observation,
        started_at,
        checked_at,
    )
    .await
}

async fn insert_codex_quota_window_period(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    observation: ObservedCodexQuotaWindow,
    started_at: DateTime<Utc>,
    checked_at: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          initial_used_percent,last_used_percent,first_observed_at,last_observed_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$7,$8,$8)",
    )
    .bind(Uuid::new_v4())
    .bind(channel_id)
    .bind(kind.as_str())
    .bind(observation.window_seconds)
    .bind(started_at)
    .bind(observation.reset_at)
    .bind(observation.used_percent)
    .bind(checked_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn claim_manual_codex_quota_reset(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    kind: CodexQuotaWindowKind,
    transition_started_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Result<bool, RepositoryError> {
    let event_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id \
         FROM codex_quota_reset_events \
         WHERE credential_id=$1 \
           AND outcome IN ('reset','already_redeemed') \
           AND windows_reset > ( \
               CASE WHEN primary_applied_at IS NULL THEN 0 ELSE 1 END \
               + CASE WHEN secondary_applied_at IS NULL THEN 0 ELSE 1 END \
           ) \
           AND requested_at <= $2 + make_interval(secs => $3) \
           AND requested_at >= $2 - make_interval(secs => $3) \
           AND CASE \
               WHEN $4='primary' THEN primary_applied_at \
               ELSE secondary_applied_at \
           END IS NULL \
         ORDER BY requested_at DESC,id DESC \
         FOR UPDATE \
         LIMIT 1",
    )
    .bind(channel_id)
    .bind(transition_started_at)
    .bind(MANUAL_RESET_MATCH_WINDOW.num_seconds())
    .bind(kind.as_str())
    .fetch_optional(&mut **transaction)
    .await?;
    let Some(event_id) = event_id else {
        return Ok(false);
    };
    let update = match kind {
        CodexQuotaWindowKind::Primary => {
            "UPDATE codex_quota_reset_events SET primary_applied_at=$2 WHERE id=$1"
        }
        CodexQuotaWindowKind::Secondary => {
            "UPDATE codex_quota_reset_events SET secondary_applied_at=$2 WHERE id=$1"
        }
    };
    sqlx::query(update)
        .bind(event_id)
        .bind(observed_at)
        .execute(&mut **transaction)
        .await?;
    Ok(true)
}

fn timestamps_within(left: DateTime<Utc>, right: DateTime<Utc>, tolerance: Duration) -> bool {
    left.signed_duration_since(right).abs() <= tolerance
}

fn credential_select(suffix: &str) -> String {
    format!(
        "SELECT c.channel_id,c.channel_group_id,c.connector_pool_id, \
                ARRAY( \
                    SELECT projection.channel_id \
                    FROM codex_oauth_credential_channels AS projection \
                    WHERE projection.credential_id=c.channel_id \
                    ORDER BY projection.api_format \
                ) AS projection_channel_ids, \
                c.label,c.email,c.account_id,c.user_id,c.plan_type, \
                c.is_fedramp,c.id_token,c.access_token,c.refresh_token, \
                c.access_token_expires_at,c.last_refreshed_at,c.refresh_generation, \
                c.reauth_required, \
                c.quota_threshold_percent,c.runtime_status,c.quota_allowed, \
                c.quota_limit_reached,c.primary_used_percent,c.primary_window_seconds, \
                c.primary_reset_at,c.secondary_used_percent,c.secondary_window_seconds, \
                c.secondary_reset_at,c.quota_reset_credits_available,c.quota_checked_at, \
                c.last_error_code,c.last_error_summary, \
                ch.proxy_id,ch.weight,c.enabled,ch.available_models,c.created_at,c.updated_at \
         FROM codex_oauth_credentials c JOIN channels ch ON ch.id=c.channel_id {suffix}"
    )
}

async fn existing_codex_channel_id(
    transaction: &mut Transaction<'_, Postgres>,
    connector_pool_id: Uuid,
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Result<Option<Uuid>, RepositoryError> {
    let Some(account_id) = account_id else {
        let Some(user_id) = user_id else {
            return Err(RepositoryError::Validation);
        };
        return sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=$1 AND account_id IS NULL AND user_id=$2 \
               AND deleted_at IS NULL \
             FOR UPDATE",
        )
        .bind(connector_pool_id)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RepositoryError::from);
    };

    if let Some(user_id) = user_id {
        let exact = sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=$1 AND account_id=$2 AND user_id=$3 \
               AND deleted_at IS NULL \
             FOR UPDATE",
        )
        .bind(connector_pool_id)
        .bind(account_id)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?;
        if exact.is_some() {
            return Ok(exact);
        }
    }

    if let Some(email) = email.map(str::trim).filter(|value| !value.is_empty()) {
        // Once the token carries a member ID, email is only a migration bridge
        // for legacy rows that have not been backfilled yet.
        let matches = sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=$1 AND account_id=$2 \
               AND lower(email)=lower($3) AND deleted_at IS NULL \
               AND ($4 OR user_id IS NULL) \
             ORDER BY channel_id \
             FOR UPDATE",
        )
        .bind(connector_pool_id)
        .bind(account_id)
        .bind(email)
        .bind(user_id.is_none())
        .fetch_all(&mut **transaction)
        .await?;
        return match matches.as_slice() {
            [] => Ok(None),
            [channel_id] => Ok(Some(*channel_id)),
            _ => Err(RepositoryError::Conflict),
        };
    }

    if user_id.is_none() {
        return sqlx::query_scalar::<_, Uuid>(
            "SELECT channel_id FROM codex_oauth_credentials \
             WHERE connector_pool_id=$1 AND account_id=$2 AND user_id IS NULL \
               AND deleted_at IS NULL \
             FOR UPDATE",
        )
        .bind(connector_pool_id)
        .bind(account_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RepositoryError::from);
    }

    Ok(None)
}

async fn set_codex_credential_enabled(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    connector_pool_id: Uuid,
    expected_updated_at: DateTime<Utc>,
    enabled: bool,
) -> Result<MutationResult, RepositoryError> {
    let before = codex_credential_audit(transaction, channel_id).await?;
    let actual_connector_pool_id = before["connector_pool_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(RepositoryError::Validation)?;
    if actual_connector_pool_id != connector_pool_id {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        "UPDATE codex_oauth_credentials SET \
         enabled=$3,runtime_status=CASE \
             WHEN NOT $3 THEN 'disabled' \
             WHEN reauth_required THEN 'unavailable' \
             WHEN quota_allowed=false OR quota_limit_reached=true THEN 'unavailable' \
             WHEN GREATEST(COALESCE(primary_used_percent,0), \
                           COALESCE(secondary_used_percent,0)) >= quota_threshold_percent \
                 THEN 'draining' \
             ELSE 'active' END \
         WHERE channel_id=$1 AND updated_at=$2 AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(channel_id)
    .bind(expected_updated_at)
    .bind(enabled)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id: channel_id,
        object_type: "codex_oauth_credential",
        action: "batch_update",
        before_redacted: before,
        after_redacted: codex_credential_audit(transaction, channel_id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn delete_codex_credential(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
    expected_connector_pool_id: Option<Uuid>,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = codex_credential_audit(transaction, channel_id).await?;
    let actual_connector_pool_id = before["connector_pool_id"]
        .as_str()
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(RepositoryError::Validation)?;
    if expected_connector_pool_id.is_some_and(|expected| expected != actual_connector_pool_id) {
        return Err(RepositoryError::NotFound);
    }
    let updated_at = sqlx::query_scalar::<_, DateTime<Utc>>(
        "UPDATE codex_oauth_credentials SET \
         enabled=false,runtime_status='disabled',reauth_required=false, \
         id_token='deleted',access_token='deleted',refresh_token='deleted', \
         access_token_expires_at=NULL,quota_allowed=NULL,quota_limit_reached=NULL, \
         primary_used_percent=NULL,primary_window_seconds=NULL,primary_reset_at=NULL, \
         secondary_used_percent=NULL,secondary_window_seconds=NULL,secondary_reset_at=NULL, \
         quota_reset_credits_available=NULL,quota_checked_at=NULL, \
         last_error_code=NULL,last_error_summary=NULL,deleted_at=now() \
         WHERE channel_id=$1 AND updated_at=$2 AND deleted_at IS NULL \
         RETURNING updated_at",
    )
    .bind(channel_id)
    .bind(expected_updated_at)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    sqlx::query("UPDATE channels SET name=$2,proxy_id=NULL WHERE id=$1")
        .bind(channel_id)
        .bind(format!("deleted-codex-{channel_id}"))
        .execute(&mut **transaction)
        .await?;
    Ok(MutationResult {
        id: channel_id,
        object_type: "codex_oauth_credential",
        action: "delete",
        before_redacted: before,
        after_redacted: json!({}),
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn validate_codex_group_and_proxy_pool(
    pool: &PgPool,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<CodexPoolContext, RepositoryError> {
    let mut connection = pool.acquire().await?;
    validate_codex_group_and_proxy_connection(&mut connection, channel_group_id, proxy_id).await
}

async fn codex_pool_context_pool(
    pool: &PgPool,
    channel_group_id: Uuid,
) -> Result<CodexPoolContext, RepositoryError> {
    let mut connection = pool.acquire().await?;
    codex_pool_context_connection(&mut connection, channel_group_id).await
}

async fn validate_codex_group_and_proxy_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<CodexPoolContext, RepositoryError> {
    validate_codex_group_and_proxy_connection(transaction, channel_group_id, proxy_id).await
}

async fn validate_codex_group_and_proxy_connection(
    connection: &mut PgConnection,
    channel_group_id: Uuid,
    proxy_id: Option<Uuid>,
) -> Result<CodexPoolContext, RepositoryError> {
    let context = codex_pool_context_connection(&mut *connection, channel_group_id).await?;
    if let Some(proxy_id) = proxy_id {
        let valid_proxy = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM proxies WHERE id=$1 AND enabled)",
        )
        .bind(proxy_id)
        .fetch_one(&mut *connection)
        .await?;
        if !valid_proxy {
            return Err(RepositoryError::Validation);
        }
    }
    Ok(context)
}

async fn codex_pool_context_connection(
    connection: &mut PgConnection,
    channel_group_id: Uuid,
) -> Result<CodexPoolContext, RepositoryError> {
    sqlx::query_as::<_, CodexPoolContext>(
        "SELECT selected.connector_pool_id, \
                responses.id AS responses_channel_group_id \
         FROM channel_groups AS selected \
         JOIN channel_groups AS responses \
           ON responses.connector_pool_id=selected.connector_pool_id \
          AND responses.api_format='open_ai_responses'::api_format \
         JOIN channel_groups AS images \
           ON images.connector_pool_id=selected.connector_pool_id \
          AND images.api_format='open_ai_images'::api_format \
         WHERE selected.id=$1 AND selected.connector_kind=$2",
    )
    .bind(channel_group_id)
    .bind(CODEX_CONNECTOR_KIND)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or(RepositoryError::Validation)
}

async fn codex_credential_audit(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
) -> Result<Value, RepositoryError> {
    sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object( \
             'id',c.channel_id,'channel_group_id',c.channel_group_id, \
             'connector_pool_id',c.connector_pool_id,'label',c.label, \
             'email',c.email,'account_id',c.account_id,'user_id',c.user_id, \
             'plan_type',c.plan_type, \
             'is_fedramp',c.is_fedramp,'access_token_expires_at',c.access_token_expires_at, \
             'last_refreshed_at',c.last_refreshed_at, \
             'quota_threshold_percent',c.quota_threshold_percent, \
             'runtime_status',c.runtime_status,'proxy_id',ch.proxy_id,'weight',ch.weight, \
             'enabled',c.enabled,'available_models',ch.available_models, \
             'projections',( \
                 SELECT json_agg( \
                     json_build_object( \
                         'api_format',projection.api_format, \
                         'channel_id',projection.channel_id, \
                         'channel_group_id',projection_channel.channel_group_id, \
                         'available_models',projection_channel.available_models, \
                         'supports_websocket',projection_channel.supports_websocket, \
                         'supports_standalone_web_search',projection_channel.supports_standalone_web_search \
                     ) ORDER BY projection.api_format \
                 ) \
                 FROM codex_oauth_credential_channels AS projection \
                 JOIN channels AS projection_channel ON projection_channel.id=projection.channel_id \
                 WHERE projection.credential_id=c.channel_id \
             ), \
             'created_at',c.created_at,'updated_at',c.updated_at) \
         FROM codex_oauth_credentials c JOIN channels ch ON ch.id=c.channel_id \
         WHERE c.channel_id=$1 AND c.deleted_at IS NULL FOR UPDATE OF c,ch",
    )
    .bind(channel_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)
}

fn validate_credential_settings(
    label: &str,
    weight: i32,
    quota_threshold_percent: i16,
) -> Result<(), RepositoryError> {
    if label.trim().is_empty()
        || label.len() > 100
        || weight <= 0
        || !(1..=100).contains(&quota_threshold_percent)
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn runtime_status_for_quota(quota: &CodexQuotaUpdate, threshold: i16) -> &'static str {
    if !quota.allowed || quota.limit_reached {
        return "unavailable";
    }
    let used = quota
        .primary_used_percent
        .unwrap_or_default()
        .max(quota.secondary_used_percent.unwrap_or_default());
    if used >= i32::from(threshold) {
        "draining"
    } else {
        "active"
    }
}

const fn default_weight() -> i32 {
    100
}

const fn default_quota_threshold_percent() -> i16 {
    95
}

const fn default_include_proxies() -> bool {
    true
}

const fn default_enabled() -> bool {
    true
}
