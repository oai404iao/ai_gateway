//! Backend-neutral Codex OAuth persistence contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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

#[derive(Clone, Serialize)]
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

#[derive(Clone)]
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

#[derive(Clone)]
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

#[derive(Clone, Debug, Serialize)]
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
    pub primary_window_cost_amount: Option<rust_decimal::Decimal>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub secondary_window_cost_amount: Option<rust_decimal::Decimal>,
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

#[derive(Clone, Debug, Serialize)]
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
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodexQuotaWindowHistory {
    pub credential_id: Uuid,
    pub periods: Vec<CodexQuotaWindowPeriodView>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfCodexQuotaCredentialView {
    pub id: Uuid,
    pub name: String,
    pub channel_group_id: Uuid,
    pub plan_type: Option<String>,
    pub primary_used_percent: Option<i32>,
    pub primary_window_seconds: Option<i32>,
    pub primary_reset_at: Option<DateTime<Utc>>,
    pub primary_window_cost_amount: Option<rust_decimal::Decimal>,
    pub secondary_used_percent: Option<i32>,
    pub secondary_window_seconds: Option<i32>,
    pub secondary_reset_at: Option<DateTime<Utc>>,
    pub secondary_window_cost_amount: Option<rust_decimal::Decimal>,
    pub quota_checked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Serialize)]
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
    pub cost_amount: rust_decimal::Decimal,
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
