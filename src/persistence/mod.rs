//! SQLx control-plane and append-only request-log repositories.

mod auth;
mod codex;

pub use auth::{
    AuthRepository, ConsoleProfile, ConsoleSession, ConsoleSessionState, InvitationCreated,
    InviteUserInput, LiveConsoleIdentity, LoginUser, PasswordUser, RegistrationAttempt,
    RegistrationInvitationCode, RegistrationInvitationCodeInput,
    RegistrationInvitationCodeMutation, SessionRotation, SessionUser,
};
pub use codex::{
    CodexCredentialBatchInput, CodexCredentialBatchOperation, CodexCredentialBatchTarget,
    CodexCredentialCreate, CodexCredentialExportBundle, CodexCredentialExportInput,
    CodexCredentialExportItem, CodexCredentialExportProxy, CodexCredentialImportInput,
    CodexCredentialRecord, CodexCredentialUpdateInput, CodexCredentialView, CodexOauthFlowRecord,
    CodexOauthStartInput, CodexQuotaUpdate, CodexTokenRefreshUpdate,
};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt,
};

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc, Weekday};
use regex::Regex;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder, Transaction, postgres::PgPoolCopyExt};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    domain::{AutomaticDisableTrigger, MAX_REQUEST_RETRIES, RequestLogEvent},
    request_log_journal::EncodedRequestLog,
};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Singleton row that supplies runtime forwarding defaults.
pub const FORWARDING_SETTINGS_KEY: &str = "forwarding_policy";
pub const SYSTEM_PROBE_USER_ID: Uuid = Uuid::from_u128(0x2c2e_3fd5_07e6_4c44_b5c7_cfe4_7bda_2b10);
pub const SYSTEM_PROBE_API_KEY_ID: Uuid =
    Uuid::from_u128(0x729d_37d8_2ad3_44ef_9e65_bf7e_410b_0f2f);
pub const DEFAULT_USER_GROUP_ID: Uuid = Uuid::from_u128(0x101);
pub const DEFAULT_ADMIN_GROUP_ID: Uuid = Uuid::from_u128(0x102);
const SYSTEM_PROBE_DISPLAY_NAME: &str = "ai-gateway-system-scheduled-tests-2c2e3fd5";
const SYSTEM_PROBE_API_KEY_NAME: &str = "system-scheduled-tests";
const SPEND_LEADERBOARD_REFRESH_LOCK: i64 = 0x5350_454E_445F_4C42;

fn forwarding_settings_object_id() -> Uuid {
    Uuid::from_u128(0x6ed3_d02b_bda1_4d85_85b9_3f9d_7362_5001)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemProbeIdentity {
    pub user_id: Uuid,
    pub api_key_id: Uuid,
}

#[derive(Debug, Default)]
pub struct ControlPlaneRecords {
    pub api_keys: Vec<ApiKeyRecord>,
    pub models: Vec<ModelRecord>,
    pub model_rules: Vec<ModelRuleRecord>,
    pub groups: Vec<ChannelGroupRecord>,
    pub channels: Vec<ChannelRecord>,
    pub proxies: Vec<ProxyRecord>,
    pub templates: Vec<ConfigTemplateRecord>,
}

/// Coherent database input for one complete runtime snapshot.
#[derive(Debug)]
pub struct RuntimeConfigRecords {
    pub control_plane: ControlPlaneRecords,
    pub system_settings: SystemSettingsRecord,
}

#[derive(Debug, FromRow)]
pub struct SystemSettingsRecord {
    pub setting_key: String,
    pub value: Value,
    pub updated_at: DateTime<Utc>,
}

/// JSON value persisted under [`FORWARDING_SETTINGS_KEY`].
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSettingsInput {
    /// User-facing HTTP(S) base URLs for the OpenAI-compatible data plane.
    #[serde(default)]
    pub api_hosts: Vec<String>,
    pub upstream: SystemUpstreamSettingsInput,
    #[serde(default)]
    pub request_retry: SystemRequestRetrySettingsInput,
    pub passive_health: SystemPassiveHealthSettingsInput,
    #[serde(default)]
    pub automatic_disable: SystemAutomaticDisableSettingsInput,
    #[serde(default)]
    pub scheduled_testing: SystemScheduledTestingSettingsInput,
    pub session_affinity: SystemSessionAffinitySettingsInput,
    #[serde(default)]
    pub websocket: SystemWebSocketSettingsInput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemUpstreamSettingsInput {
    pub connect_timeout_seconds: u64,
    pub response_header_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemRequestRetrySettingsInput {
    #[serde(default = "default_request_retry_enabled")]
    pub enabled: bool,
    #[serde(default = "default_request_retry_max_retries")]
    pub max_retries: u32,
}

impl Default for SystemRequestRetrySettingsInput {
    fn default() -> Self {
        Self {
            enabled: default_request_retry_enabled(),
            max_retries: default_request_retry_max_retries(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemPassiveHealthSettingsInput {
    pub connection_failure_threshold: u32,
    pub cooldown_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemAutomaticDisableSettingsInput {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub error_status_codes: Vec<u16>,
    #[serde(default)]
    pub error_message_keywords: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemScheduledTestingSettingsInput {
    #[serde(default = "default_scheduled_testing_mode")]
    pub mode: String,
    #[serde(default = "default_scheduled_testing_auto_recover")]
    pub auto_recover: bool,
    #[serde(default = "default_scheduled_testing_interval_minutes")]
    pub interval_minutes: u64,
    #[serde(default = "default_scheduled_testing_prompt")]
    pub prompt: String,
}

impl Default for SystemScheduledTestingSettingsInput {
    fn default() -> Self {
        Self {
            mode: default_scheduled_testing_mode(),
            auto_recover: default_scheduled_testing_auto_recover(),
            interval_minutes: default_scheduled_testing_interval_minutes(),
            prompt: default_scheduled_testing_prompt(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSessionAffinitySettingsInput {
    pub enabled: bool,
    pub max_entries: usize,
    pub default_ttl_seconds: u64,
    pub rules: Vec<SystemSessionAffinityRuleInput>,
}

impl Default for SystemSessionAffinitySettingsInput {
    fn default() -> Self {
        Self {
            enabled: false,
            max_entries: 100_000,
            default_ttl_seconds: 3_600,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemWebSocketSettingsInput {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_websocket_max_idle_connections")]
    pub max_idle_connections: usize,
    #[serde(default = "default_websocket_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
    #[serde(default = "default_websocket_max_connection_age_seconds")]
    pub max_connection_age_seconds: u64,
}

impl Default for SystemWebSocketSettingsInput {
    fn default() -> Self {
        Self {
            enabled: false,
            max_idle_connections: default_websocket_max_idle_connections(),
            idle_timeout_seconds: default_websocket_idle_timeout_seconds(),
            max_connection_age_seconds: default_websocket_max_connection_age_seconds(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemSessionAffinityRuleInput {
    pub name: String,
    pub enabled: bool,
    pub api_formats: Vec<String>,
    pub model_regex: Vec<String>,
    pub key_sources: Vec<SystemSessionAffinityKeySourceInput>,
    pub value_regex: Option<String>,
    pub ttl_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SystemSessionAffinityKeySourceInput {
    RequestHeader { name: String },
    JsonPointer { pointer: String },
}

fn default_scheduled_testing_mode() -> String {
    "global".into()
}

const fn default_request_retry_enabled() -> bool {
    true
}

const fn default_request_retry_max_retries() -> u32 {
    1
}

const fn default_scheduled_testing_auto_recover() -> bool {
    true
}

const fn default_scheduled_testing_interval_minutes() -> u64 {
    5
}

fn default_scheduled_testing_prompt() -> String {
    "reply '1'".into()
}

const fn default_websocket_max_idle_connections() -> usize {
    128
}

const fn default_websocket_idle_timeout_seconds() -> u64 {
    5 * 60
}

const fn default_websocket_max_connection_age_seconds() -> u64 {
    55 * 60
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemSettingsView {
    #[serde(flatten)]
    pub settings: SystemSettingsInput,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct ApiHostsView {
    pub api_hosts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct UserSettingsView {
    pub websocket_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettingsInput {
    pub websocket_enabled: bool,
}

#[derive(FromRow)]
pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub user_websocket_enabled: bool,
    pub secret_value: String,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub allowed_api_formats: Vec<String>,
    pub permissions: Vec<String>,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub requests_per_minute: Option<i32>,
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
            .field("user_websocket_enabled", &self.user_websocket_enabled)
            .field("secret_value", &"REDACTED")
            .field("status", &self.status)
            .field("expires_at", &self.expires_at)
            .field("allowed_api_formats", &self.allowed_api_formats)
            .field("permissions", &self.permissions)
            .field("allowed_group_ids", &self.allowed_group_ids)
            .field("allowed_channel_ids", &self.allowed_channel_ids)
            .field("requests_per_minute", &self.requests_per_minute)
            .field("max_concurrent_requests", &self.max_concurrent_requests)
            .field("quota_limit_amount", &self.quota_limit_amount)
            .field("quota_used_amount", &self.quota_used_amount)
            .finish()
    }
}
#[derive(Clone, Debug, FromRow)]
pub struct ModelRecord {
    pub id: Uuid,
    pub source_model_id: String,
    pub currency: String,
    pub price_unit_tokens: i64,
    pub price_effective_at: DateTime<Utc>,
    pub input_unit_price: rust_decimal::Decimal,
    pub cached_input_unit_price: rust_decimal::Decimal,
    pub cache_write_unit_price: rust_decimal::Decimal,
    pub output_unit_price: rust_decimal::Decimal,
    pub advanced_billing: Value,
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
    pub advanced_billing: Value,
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
    pub connector_kind: String,
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
    pub supports_websocket: bool,
    pub auto_disabled: bool,
    pub auto_disable_allowed: bool,
    pub weight: i32,
    pub billing_multiplier: rust_decimal::Decimal,
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
    pub test_model: Option<String>,
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
            .field("supports_websocket", &self.supports_websocket)
            .field("auto_disabled", &self.auto_disabled)
            .field("auto_disable_allowed", &self.auto_disable_allowed)
            .field("weight", &self.weight)
            .field("billing_multiplier", &self.billing_multiplier)
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
            .field("test_model", &self.test_model)
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
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
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
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub expires_at: Option<DateTime<Utc>>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}

/// Administrator-owned bounds for the routing targets a user may choose.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyPolicyInput {
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub enabled: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserGroupInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_api_key_policy_id: Option<Uuid>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyCreate {
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requests_per_minute: Option<i32>,
    #[serde(default)]
    pub max_concurrent_requests: Option<i32>,
    #[serde(default)]
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelfApiKeyUpdate {
    pub name: String,
    pub status: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requests_per_minute: Option<i32>,
    #[serde(default)]
    pub max_concurrent_requests: Option<i32>,
    #[serde(default)]
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserInput {
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default = "default_user_role")]
    pub role: String,
    pub status: String,
    pub balance_amount: rust_decimal::Decimal,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default)]
    pub default_api_key_policy_id: Option<Uuid>,
}

/// Partial administrator update for a Console user.
///
/// Omitted fields preserve their current value. Nullable fields use a nested
/// option so an explicit JSON `null` can clear the value while omission leaves
/// it unchanged.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserUpdateInput {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub email: Option<Option<String>>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub balance_amount: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_uuid")]
    pub default_api_key_policy_id: Option<Option<Uuid>>,
}

impl UserUpdateInput {
    fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.email.is_none()
            && self.role.is_none()
            && self.status.is_none()
            && self.balance_amount.is_none()
            && self.user_group_id.is_none()
            && self.default_api_key_policy_id.is_none()
    }
}

impl From<UserInput> for UserUpdateInput {
    fn from(value: UserInput) -> Self {
        Self {
            display_name: Some(value.display_name),
            email: Some(value.email),
            role: Some(value.role),
            status: Some(value.status),
            balance_amount: Some(value.balance_amount),
            user_group_id: value.user_group_id,
            default_api_key_policy_id: Some(value.default_api_key_policy_id),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchUpdateInput {
    pub items: Vec<UserBatchUpdateTarget>,
    pub changes: UserBatchChanges,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchUpdateTarget {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBalanceBatchChange {
    pub operation: String,
    pub amount: rust_decimal::Decimal,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserBatchChanges {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub balance: Option<UserBalanceBatchChange>,
    #[serde(default)]
    pub user_group_id: Option<Uuid>,
    #[serde(default, deserialize_with = "deserialize_optional_uuid")]
    pub default_api_key_policy_id: Option<Option<Uuid>>,
}

impl UserBatchChanges {
    fn is_empty(&self) -> bool {
        self.status.is_none()
            && self.balance.is_none()
            && self.user_group_id.is_none()
            && self.default_api_key_policy_id.is_none()
    }
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
    /// Create defaults to no advanced billing; omission during an update
    /// preserves the model's existing policy.
    #[serde(default)]
    pub advanced_billing: Option<crate::domain::AdvancedBilling>,
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
    pub advanced_billing: crate::domain::AdvancedBilling,
    pub source_payload: Value,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelGroupInput {
    pub name: String,
    pub api_format: String,
    #[serde(default = "default_connector_kind")]
    pub connector_kind: String,
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
    pub supports_websocket: bool,
    #[serde(default)]
    pub status_statistics_enabled: bool,
    #[serde(default)]
    pub auto_disable_allowed: bool,
    pub weight: i32,
    #[serde(default = "default_billing_multiplier")]
    pub billing_multiplier: rust_decimal::Decimal,
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
    #[serde(default)]
    pub test_model: Option<String>,
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
    pub supports_websocket: bool,
    #[serde(default)]
    pub status_statistics_enabled: bool,
    #[serde(default)]
    pub auto_disable_allowed: bool,
    pub weight: i32,
    /// Omission preserves the current multiplier.
    #[serde(default)]
    pub billing_multiplier: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub proxy_id: Option<Uuid>,
    #[serde(default)]
    pub config_template_id: Option<Uuid>,
    /// Omission preserves the current transform document; a present value
    /// replaces it (including `{}` to clear it).
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
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub upstream_api_key: Option<Option<String>>,
    #[serde(default)]
    pub available_models: Vec<String>,
    #[serde(default)]
    pub test_model: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchUpdateInput {
    pub items: Vec<ChannelBatchUpdateTarget>,
    pub changes: ChannelBatchChanges,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchUpdateTarget {
    pub id: Uuid,
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRecoverInput {
    pub updated_at: DateTime<Utc>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelBatchChanges {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub status_statistics_enabled: Option<bool>,
    #[serde(default)]
    pub auto_disable_allowed: Option<bool>,
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub billing_multiplier: Option<rust_decimal::Decimal>,
}
impl ChannelBatchChanges {
    fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.status_statistics_enabled.is_none()
            && self.auto_disable_allowed.is_none()
            && self.weight.is_none()
            && self.billing_multiplier.is_none()
    }
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
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub username: Option<Option<String>>,
    /// Absent keeps the current credential component; null explicitly clears it.
    #[serde(default, deserialize_with = "deserialize_optional_string")]
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
    /// Omission preserves the stored document; a present value replaces it.
    #[serde(default, deserialize_with = "deserialize_optional_document")]
    pub document: Option<Value>,
    pub enabled: bool,
}
fn empty_object() -> Value {
    json!({})
}
fn default_billing_multiplier() -> rust_decimal::Decimal {
    rust_decimal::Decimal::ONE
}
fn default_connector_kind() -> String {
    "openai_compatible".into()
}
fn deserialize_optional_string<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}
fn deserialize_optional_uuid<'de, D>(deserializer: D) -> Result<Option<Option<Uuid>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<Uuid>::deserialize(deserializer).map(Some)
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
    supports_websocket: bool,
    status_statistics_enabled: bool,
    auto_disable_allowed: bool,
    weight: i32,
    billing_multiplier: Option<rust_decimal::Decimal>,
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
    test_model: Option<String>,
}
impl From<ChannelCreateInput> for ChannelMutationInput {
    fn from(value: ChannelCreateInput) -> Self {
        Self {
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            supports_websocket: value.supports_websocket,
            status_statistics_enabled: value.status_statistics_enabled,
            auto_disable_allowed: value.auto_disable_allowed,
            weight: value.weight,
            billing_multiplier: Some(value.billing_multiplier),
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
            test_model: value.test_model,
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
            supports_websocket: value.supports_websocket,
            status_statistics_enabled: value.status_statistics_enabled,
            auto_disable_allowed: value.auto_disable_allowed,
            weight: value.weight,
            billing_multiplier: value.billing_multiplier,
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
            test_model: value.test_model,
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
        input: UserUpdateInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteUser {
        id: Uuid,
        deleted_by: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateUserGroup(UserGroupInput),
    UpdateUserGroup {
        id: Uuid,
        input: UserGroupInput,
        expected_updated_at: DateTime<Utc>,
    },
    DeleteUserGroup {
        id: Uuid,
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
    RecoverChannel {
        id: Uuid,
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
    DeleteProxy {
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    },
    CreateConfigTemplate(ConfigTemplateCreateInput),
    UpdateConfigTemplate {
        id: Uuid,
        input: ConfigTemplateInput,
        expected_updated_at: DateTime<Utc>,
    },
    UpdateSystemSettings {
        input: SystemSettingsInput,
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
    pub user_groups: Vec<ControlPlaneUserGroup>,
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
    pub can_reissue_invitation: bool,
    pub user_group_id: Uuid,
    pub default_api_key_policy_id: Option<Uuid>,
    pub effective_api_key_policy_id: Option<Uuid>,
    pub balance_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneUserGroup {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub default_api_key_policy_id: Option<Uuid>,
    pub system_role: Option<String>,
    pub member_count: i64,
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
    pub advanced_billing: Value,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneApiKeyPolicy {
    pub id: Uuid,
    pub name: String,
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
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
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub requests_per_minute: Option<i32>,
    pub max_concurrent_requests: Option<i32>,
    pub quota_limit_amount: Option<rust_decimal::Decimal>,
    pub quota_used_amount: rust_decimal::Decimal,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct SelfApiKeyOptions {
    pub policy_id: Uuid,
    pub policy_name: String,
    pub groups: Vec<SelfApiKeyGroupOption>,
    pub channels: Vec<SelfApiKeyChannelOption>,
}

#[derive(Clone, Serialize, FromRow)]
pub struct SelfApiKeyGroupOption {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub enabled: bool,
}

#[derive(Clone, Serialize, FromRow)]
pub struct SelfApiKeyChannelOption {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub api_format: String,
    pub name: String,
    pub enabled: bool,
    pub auto_disabled: bool,
}

#[derive(Clone, Debug, Serialize, FromRow)]
pub struct ConsoleRequestLog {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub user_name: Option<String>,
    pub api_key_id: Uuid,
    pub request_source: String,
    pub api_format: String,
    pub request_protocol: String,
    pub client_model: String,
    pub reasoning_effort: Option<String>,
    pub fast_mode: bool,
    pub upstream_model: Option<String>,
    pub model_rule_id: Option<Uuid>,
    pub channel_group_id: Option<Uuid>,
    pub channel_group_name: Option<String>,
    pub channel_id: Option<Uuid>,
    pub channel_name: Option<String>,
    pub outcome: String,
    pub response_status_code: Option<i16>,
    pub streamed: bool,
    pub ttft_ms: Option<i32>,
    pub total_duration_ms: Option<i32>,
    pub output_tokens_per_second: Option<rust_decimal::Decimal>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cost_amount: Option<rust_decimal::Decimal>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
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
    pub channel_id: Option<Uuid>,
    pub include_channel_details: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpendLeaderboardPeriod {
    Day,
    Week,
    Month,
}

impl SpendLeaderboardPeriod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    #[must_use]
    pub fn current_start_at(self, now: DateTime<Utc>) -> NaiveDate {
        // Asia/Shanghai is UTC+08:00 and does not observe daylight saving time.
        let today = (now + chrono::Duration::hours(8)).date_naive();
        match self {
            Self::Day => today,
            Self::Week => {
                today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()))
            }
            Self::Month => today
                .with_day(1)
                .expect("every calendar month has a first day"),
        }
    }

    #[must_use]
    pub fn end_after(self, period_start: NaiveDate) -> NaiveDate {
        match self {
            Self::Day => period_start
                .succ_opt()
                .expect("a supported date always has a next day"),
            Self::Week => period_start
                .checked_add_signed(chrono::Duration::days(7))
                .expect("a supported date always has a following week"),
            Self::Month => {
                let (year, month) = if period_start.month() == 12 {
                    (period_start.year() + 1, 1)
                } else {
                    (period_start.year(), period_start.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1)
                    .expect("a supported date always has a following month")
            }
        }
    }

    #[must_use]
    pub fn is_valid_start(self, period_start: NaiveDate) -> bool {
        match self {
            Self::Day => true,
            Self::Week => period_start.weekday() == Weekday::Mon,
            Self::Month => period_start.day() == 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpendLeaderboardFilter {
    pub period: SpendLeaderboardPeriod,
    pub period_start: NaiveDate,
    pub limit: i64,
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
pub struct PersonalUsageReport {
    pub started_on: NaiveDate,
    pub ended_on: NaiveDate,
    pub total_request_count: i64,
    pub active_day_count: i64,
    pub current_streak_days: i64,
    pub longest_streak_days: i64,
    pub days: Vec<PersonalUsageDay>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PersonalUsageDay {
    pub date: NaiveDate,
    pub request_count: i64,
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
    pub channels: Vec<CostStatisticsChannel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsSummary {
    pub request_count: i64,
    pub priced_request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
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
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub success_rate: Option<f64>,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub name: String,
    pub api_format: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub success_rate: Option<f64>,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendLeaderboardReport {
    pub period: String,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub total_cost_amount: rust_decimal::Decimal,
    pub previous_period_start: Option<NaiveDate>,
    pub next_period_start: Option<NaiveDate>,
    pub entries: Vec<SpendLeaderboardEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpendLeaderboardRefresh {
    Updated,
    AlreadyRunning,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendLeaderboardEntry {
    pub rank: i64,
    pub user_id: Uuid,
    pub display_name: String,
    pub request_count: i64,
    pub priced_request_count: i64,
    pub total_tokens: i64,
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
    pub allowed_group_ids: Vec<Uuid>,
    pub allowed_channel_ids: Vec<Uuid>,
    pub requests_per_minute: Option<i32>,
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
    pub connector_kind: String,
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
    pub connector_kind: String,
    pub provider_managed: bool,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub supports_websocket: bool,
    pub status_statistics_enabled: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub auto_disable_allowed: bool,
    pub weight: i32,
    pub billing_multiplier: rust_decimal::Decimal,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_credential_configured: bool,
    pub available_models: Vec<String>,
    pub test_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Serialize, FromRow)]
pub struct ControlPlaneChannelDetail {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub connector_kind: String,
    pub provider_managed: bool,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub supports_websocket: bool,
    pub status_statistics_enabled: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub auto_disable_allowed: bool,
    pub weight: i32,
    pub billing_multiplier: rust_decimal::Decimal,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub override_document: Value,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub upstream_auth_kind: String,
    pub upstream_auth_header_name: Option<String>,
    pub upstream_api_key: Option<String>,
    pub upstream_credential_configured: bool,
    pub available_models: Vec<String>,
    pub test_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(FromRow)]
struct ControlPlaneChannelRow {
    id: Uuid,
    channel_group_id: Uuid,
    api_format: String,
    connector_kind: String,
    provider_managed: bool,
    name: String,
    base_url: String,
    enabled: bool,
    supports_websocket: bool,
    status_statistics_enabled: bool,
    auto_disabled: bool,
    auto_disabled_reason: Option<String>,
    auto_disable_allowed: bool,
    weight: i32,
    billing_multiplier: rust_decimal::Decimal,
    proxy_id: Option<Uuid>,
    config_template_id: Option<Uuid>,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
    upstream_auth_kind: String,
    upstream_auth_header_name: Option<String>,
    upstream_credential_configured: bool,
    available_models: Vec<String>,
    test_model: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}
impl From<ControlPlaneChannelRow> for ControlPlaneChannel {
    fn from(value: ControlPlaneChannelRow) -> Self {
        Self {
            id: value.id,
            channel_group_id: value.channel_group_id,
            api_format: value.api_format,
            connector_kind: value.connector_kind,
            provider_managed: value.provider_managed,
            name: value.name,
            base_url: value.base_url,
            enabled: value.enabled,
            supports_websocket: value.supports_websocket,
            status_statistics_enabled: value.status_statistics_enabled,
            auto_disabled: value.auto_disabled,
            auto_disabled_reason: value.auto_disabled_reason,
            auto_disable_allowed: value.auto_disable_allowed,
            weight: value.weight,
            billing_multiplier: value.billing_multiplier,
            proxy_id: value.proxy_id,
            config_template_id: value.config_template_id,
            connect_timeout_ms: value.connect_timeout_ms,
            response_header_timeout_ms: value.response_header_timeout_ms,
            stream_idle_timeout_ms: value.stream_idle_timeout_ms,
            upstream_auth_kind: value.upstream_auth_kind,
            upstream_auth_header_name: value.upstream_auth_header_name,
            upstream_credential_configured: value.upstream_credential_configured,
            available_models: value.available_models,
            test_model: value.test_model,
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
#[derive(Serialize, FromRow)]
pub struct ControlPlaneConfigTemplateDetail {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub api_format: Option<String>,
    pub document: Value,
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

    /// Appends encoded terminal events to the low-index durable ingress table.
    ///
    /// Duplicates are intentionally allowed here. A checkpoint replay can
    /// therefore use PostgreSQL COPY directly, while the final request_logs
    /// primary key remains the idempotency boundary.
    pub(crate) async fn copy_ingest_batch(
        &self,
        records: &[EncodedRequestLog],
    ) -> Result<u64, RepositoryError> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut data =
            Vec::with_capacity(records.iter().map(|record| record.payload.len() + 64).sum());
        for record in records {
            data.extend_from_slice(record.request_log_id.to_string().as_bytes());
            data.push(b'\t');
            data.extend_from_slice(record.schema_version.to_string().as_bytes());
            data.push(b'\t');
            append_copy_bytea(&mut data, &record.payload);
            data.push(b'\n');
        }
        let mut copy = self
            .pool
            .copy_in_raw(
                "COPY request_log_ingest (request_log_id,schema_version,payload) \
                 FROM STDIN WITH (FORMAT text)",
            )
            .await?;
        if let Err(error) = copy.send(data.as_slice()).await {
            let _ = copy.abort("request-log ingress copy failed").await;
            return Err(error.into());
        }
        copy.finish().await.map_err(RepositoryError::from)
    }

    pub(crate) async fn load_ingest_batch(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        sqlx::query_as::<_, RequestLogIngestRecord>(
            "SELECT sequence,request_log_id,schema_version,payload,attempt_count
             FROM request_log_ingest
             WHERE next_attempt_at <= now()
             ORDER BY sequence
             LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub(crate) async fn acknowledge_ingest(
        &self,
        sequences: &[i64],
    ) -> Result<u64, RepositoryError> {
        if sequences.is_empty() {
            return Ok(0);
        }
        sqlx::query("DELETE FROM request_log_ingest WHERE sequence = ANY($1)")
            .bind(sequences)
            .execute(&self.pool)
            .await
            .map(|result| result.rows_affected())
            .map_err(RepositoryError::from)
    }

    pub(crate) async fn defer_ingest(
        &self,
        sequences: &[i64],
        error_code: &str,
        retry_after_seconds: i64,
    ) -> Result<u64, RepositoryError> {
        if sequences.is_empty() {
            return Ok(0);
        }
        sqlx::query(
            "UPDATE request_log_ingest
             SET attempt_count = attempt_count + 1,
                 next_attempt_at = now() + make_interval(secs => $2),
                 last_error_code = $3
             WHERE sequence = ANY($1)",
        )
        .bind(sequences)
        .bind(retry_after_seconds.max(1))
        .bind(error_code)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected())
        .map_err(RepositoryError::from)
    }

    pub(crate) async fn ingest_backlog(&self) -> Result<RequestLogIngestBacklog, RepositoryError> {
        sqlx::query_as::<_, RequestLogIngestBacklog>(
            "SELECT
                 COALESCE(
                     (SELECT sequence FROM request_log_ingest ORDER BY sequence DESC LIMIT 1)
                     - (SELECT sequence FROM request_log_ingest ORDER BY sequence LIMIT 1)
                     + 1,
                     0
                 ) AS row_count,
                 (
                     SELECT staged_at
                     FROM request_log_ingest
                     ORDER BY sequence
                     LIMIT 1
                 ) AS oldest_staged_at",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub(crate) async fn settlement_backlog(
        &self,
    ) -> Result<RequestLogSettlementBacklog, RepositoryError> {
        sqlx::query_as::<_, RequestLogSettlementBacklog>(
            "SELECT
                 count(*)::bigint AS row_count,
                 min(log.completed_at) AS oldest_completed_at
             FROM request_logs AS log
             JOIN api_keys AS key
               ON key.id = log.api_key_id
              AND key.user_id = log.user_id
             WHERE log.billed_at IS NULL
               AND log.cost_amount IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    #[must_use]
    pub(crate) fn pool_status(&self) -> RequestLogPoolStatus {
        RequestLogPoolStatus {
            size: self.pool.size(),
            idle: self.pool.num_idle(),
        }
    }

    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        let mut logs = query_console_request_logs(&self.pool, Some(user_id), filter).await?;
        for log in &mut logs {
            redact_self_service_request_log(log);
        }
        Ok(logs)
    }

    pub async fn get_for_user(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        let mut log = query_console_request_log(&self.pool, id, Some(user_id)).await?;
        if let Some(log) = &mut log {
            redact_self_service_request_log(log);
        }
        Ok(log)
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

    pub async fn personal_usage(
        &self,
        user_id: Uuid,
        ended_on: NaiveDate,
    ) -> Result<PersonalUsageReport, RepositoryError> {
        let started_on = ended_on
            .checked_sub_signed(chrono::Duration::days(364))
            .ok_or(RepositoryError::Validation)?;
        let ended_exclusive_on = ended_on.succ_opt().ok_or(RepositoryError::Validation)?;
        let started_at: DateTime<Utc> = DateTime::from_naive_utc_and_offset(
            started_on
                .and_hms_opt(0, 0, 0)
                .ok_or(RepositoryError::Validation)?,
            Utc,
        );
        let ended_at: DateTime<Utc> = DateTime::from_naive_utc_and_offset(
            ended_exclusive_on
                .and_hms_opt(0, 0, 0)
                .ok_or(RepositoryError::Validation)?,
            Utc,
        );
        let rows = sqlx::query_as::<_, PersonalUsageDayRow>(
            "SELECT (started_at AT TIME ZONE 'UTC')::date AS date,
                    count(*)::bigint AS request_count
             FROM request_logs
             WHERE user_id = $1
               AND request_source = 'client'
               AND started_at >= $2
               AND started_at < $3
             GROUP BY (started_at AT TIME ZONE 'UTC')::date
             ORDER BY (started_at AT TIME ZONE 'UTC')::date",
        )
        .bind(user_id)
        .bind(started_at)
        .bind(ended_at)
        .fetch_all(&self.pool)
        .await?;

        Ok(fold_personal_usage(rows, started_on, ended_on))
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
                    COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
                    COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input_tokens,
                    COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens,
                    COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
                    COALESCE(sum(cost_amount), 0) AS cost_amount
             FROM request_logs
             WHERE started_at >= $1
               AND started_at < $2
               AND ($3::uuid IS NULL OR user_id = $3)
               AND ($4::uuid IS NULL OR api_key_id = $4)
               AND ($5::uuid IS NULL OR channel_id = $5)",
        )
        .bind(filter.started_at)
        .bind(filter.ended_at)
        .bind(filter.user_id)
        .bind(filter.api_key_id)
        .bind(filter.channel_id)
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
               AND ($5::uuid IS NULL OR channel_id = $5)
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
            .bind(filter.channel_id)
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
                    COALESCE(sum(input_tokens), 0)::bigint AS input_tokens,
                    COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input_tokens,
                    COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write_tokens,
                    COALESCE(sum(output_tokens), 0)::bigint AS output_tokens,
                    COALESCE(sum(cost_amount), 0) AS cost_amount
             FROM request_logs
             WHERE started_at >= $1
               AND started_at < $2
               AND ($3::uuid IS NULL OR user_id = $3)
               AND ($4::uuid IS NULL OR api_key_id = $4)
               AND ($5::uuid IS NULL OR channel_id = $5)
             GROUP BY COALESCE(upstream_model, client_model), api_format
             ORDER BY COALESCE(upstream_model, client_model), api_format",
        )
        .bind(filter.started_at)
        .bind(filter.ended_at)
        .bind(filter.user_id)
        .bind(filter.api_key_id)
        .bind(filter.channel_id)
        .fetch_all(&self.pool)
        .await?;

        let channels = if filter.include_channel_details {
            sqlx::query_as::<_, CostChannelMetricRow>(
                "SELECT log.channel_id AS id,
                        log.channel_group_id,
                        channel_group.name AS channel_group_name,
                        channel.name,
                        log.api_format::text AS api_format,
                        count(*)::bigint AS request_count,
                        count(*) FILTER (WHERE log.outcome <> 'cancelled')::bigint
                            AS success_rate_request_count,
                        count(*) FILTER (WHERE log.outcome = 'succeeded')::bigint
                            AS succeeded_count,
                        COALESCE(
                            sum(COALESCE(log.input_tokens, 0) + COALESCE(log.output_tokens, 0)),
                            0
                        )::bigint AS total_tokens,
                        COALESCE(sum(log.input_tokens), 0)::bigint AS input_tokens,
                        COALESCE(sum(log.cached_input_tokens), 0)::bigint
                            AS cached_input_tokens,
                        COALESCE(sum(log.cache_write_tokens), 0)::bigint
                            AS cache_write_tokens,
                        COALESCE(sum(log.output_tokens), 0)::bigint AS output_tokens,
                        COALESCE(sum(log.cost_amount), 0) AS cost_amount
                 FROM request_logs AS log
                 JOIN channels AS channel ON channel.id = log.channel_id
                 JOIN channel_groups AS channel_group
                   ON channel_group.id = log.channel_group_id
                 WHERE log.started_at >= $1
                   AND log.started_at < $2
                   AND ($3::uuid IS NULL OR log.user_id = $3)
                   AND ($4::uuid IS NULL OR log.api_key_id = $4)
                   AND ($5::uuid IS NULL OR log.channel_id = $5)
                 GROUP BY log.channel_id, log.channel_group_id, channel_group.name,
                          channel.name, log.api_format
                 ORDER BY channel_group.name, channel.name, log.api_format",
            )
            .bind(filter.started_at)
            .bind(filter.ended_at)
            .bind(filter.user_id)
            .bind(filter.api_key_id)
            .bind(filter.channel_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(CostChannelMetricRow::into_metric)
            .collect()
        } else {
            Vec::new()
        };

        let duration_minutes = duration.num_milliseconds().max(1) as f64 / 60_000.0;
        Ok(CostStatisticsReport {
            started_at: filter.started_at,
            ended_at: filter.ended_at,
            granularity: filter.granularity.as_str().into(),
            summary: CostStatisticsSummary {
                request_count: summary.request_count,
                priced_request_count: summary.priced_request_count,
                total_tokens: summary.total_tokens,
                input_tokens: summary.input_tokens,
                cached_input_tokens: summary.cached_input_tokens,
                cache_write_tokens: summary.cache_write_tokens,
                output_tokens: summary.output_tokens,
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
            channels,
        })
    }

    /// Rebuilds Asia/Shanghai day, ISO-week, and calendar-month user-spend
    /// snapshots from immutable request logs. Console reads only these
    /// snapshot tables; no request-time leaderboard aggregate touches
    /// `request_logs`.
    pub async fn refresh_spend_leaderboard_snapshots(
        &self,
    ) -> Result<SpendLeaderboardRefresh, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
            .bind(SPEND_LEADERBOARD_REFRESH_LOCK)
            .fetch_one(&mut *transaction)
            .await?;
        if !acquired {
            transaction.rollback().await?;
            return Ok(SpendLeaderboardRefresh::AlreadyRunning);
        }

        sqlx::query(
            "WITH local_today AS (
                 SELECT (CURRENT_TIMESTAMP AT TIME ZONE 'Asia/Shanghai')::date AS value
             ),
             current_periods AS (
                 SELECT 'day'::text AS period,
                        value AS period_start,
                        value + 1 AS period_end
                 FROM local_today
                 UNION ALL
                 SELECT 'week',
                        date_trunc('week', value::timestamp)::date,
                        date_trunc('week', value::timestamp)::date + 7
                 FROM local_today
                 UNION ALL
                 SELECT 'month',
                        date_trunc('month', value::timestamp)::date,
                        (
                            date_trunc('month', value::timestamp)
                            + INTERVAL '1 month'
                        )::date
                 FROM local_today
             )
             INSERT INTO spend_leaderboard_periods (
                 period,
                 period_start,
                 period_end,
                 refreshed_at,
                 total_cost_amount
             )
             SELECT period, period_start, period_end, CURRENT_TIMESTAMP, 0
             FROM current_periods
             ON CONFLICT (period, period_start) DO UPDATE
             SET refreshed_at = EXCLUDED.refreshed_at",
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "CREATE TEMP TABLE spend_leaderboard_refresh_rows ON COMMIT DROP AS
             SELECT 'day'::text AS period,
                    (log.started_at AT TIME ZONE 'Asia/Shanghai')::date AS period_start,
                    log.user_id,
                    count(*)::bigint AS request_count,
                    count(log.cost_amount)::bigint AS priced_request_count,
                    COALESCE(
                        sum(COALESCE(log.input_tokens, 0) + COALESCE(log.output_tokens, 0)),
                        0
                    )::bigint AS total_tokens,
                    COALESCE(sum(log.cost_amount), 0) AS cost_amount
             FROM request_logs AS log
             WHERE log.request_source = 'client'
             GROUP BY (log.started_at AT TIME ZONE 'Asia/Shanghai')::date, log.user_id
             HAVING count(log.cost_amount) > 0

             UNION ALL

             SELECT 'week',
                    date_trunc(
                        'week',
                        log.started_at AT TIME ZONE 'Asia/Shanghai'
                    )::date,
                    log.user_id,
                    count(*)::bigint,
                    count(log.cost_amount)::bigint,
                    COALESCE(
                        sum(COALESCE(log.input_tokens, 0) + COALESCE(log.output_tokens, 0)),
                        0
                    )::bigint,
                    COALESCE(sum(log.cost_amount), 0)
             FROM request_logs AS log
             WHERE log.request_source = 'client'
             GROUP BY date_trunc(
                 'week',
                 log.started_at AT TIME ZONE 'Asia/Shanghai'
             )::date, log.user_id
             HAVING count(log.cost_amount) > 0

             UNION ALL

             SELECT 'month',
                    date_trunc(
                        'month',
                        log.started_at AT TIME ZONE 'Asia/Shanghai'
                    )::date,
                    log.user_id,
                    count(*)::bigint,
                    count(log.cost_amount)::bigint,
                    COALESCE(
                        sum(COALESCE(log.input_tokens, 0) + COALESCE(log.output_tokens, 0)),
                        0
                    )::bigint,
                    COALESCE(sum(log.cost_amount), 0)
             FROM request_logs AS log
             WHERE log.request_source = 'client'
             GROUP BY date_trunc(
                 'month',
                 log.started_at AT TIME ZONE 'Asia/Shanghai'
             )::date, log.user_id
             HAVING count(log.cost_amount) > 0",
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO spend_leaderboard_periods (
                 period,
                 period_start,
                 period_end,
                 refreshed_at,
                 total_cost_amount
             )
             SELECT period,
                    period_start,
                    CASE period
                        WHEN 'day' THEN period_start + 1
                        WHEN 'week' THEN period_start + 7
                        ELSE (period_start + INTERVAL '1 month')::date
                    END,
                    CURRENT_TIMESTAMP,
                    sum(cost_amount)
             FROM spend_leaderboard_refresh_rows
             GROUP BY period, period_start
             ON CONFLICT (period, period_start) DO UPDATE
             SET period_end = EXCLUDED.period_end,
                 refreshed_at = EXCLUDED.refreshed_at,
                 total_cost_amount = EXCLUDED.total_cost_amount",
        )
        .execute(&mut *transaction)
        .await?;

        sqlx::query(
            "INSERT INTO spend_leaderboard_entries (
                 period,
                 period_start,
                 user_id,
                 rank,
                 request_count,
                 priced_request_count,
                 total_tokens,
                 cost_amount
             )
             SELECT period,
                    period_start,
                    user_id,
                    row_number() OVER (
                        PARTITION BY period, period_start
                        ORDER BY cost_amount DESC, request_count DESC, user_id
                    )::bigint,
                    request_count,
                    priced_request_count,
                    total_tokens,
                    cost_amount
             FROM spend_leaderboard_refresh_rows
             ON CONFLICT (period, period_start, user_id) DO UPDATE
             SET rank = EXCLUDED.rank,
                 request_count = EXCLUDED.request_count,
                 priced_request_count = EXCLUDED.priced_request_count,
                 total_tokens = EXCLUDED.total_tokens,
                 cost_amount = EXCLUDED.cost_amount
             WHERE (
                 spend_leaderboard_entries.rank,
                 spend_leaderboard_entries.request_count,
                 spend_leaderboard_entries.priced_request_count,
                 spend_leaderboard_entries.total_tokens,
                 spend_leaderboard_entries.cost_amount
             ) IS DISTINCT FROM (
                 EXCLUDED.rank,
                 EXCLUDED.request_count,
                 EXCLUDED.priced_request_count,
                 EXCLUDED.total_tokens,
                 EXCLUDED.cost_amount
             )",
        )
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;
        Ok(SpendLeaderboardRefresh::Updated)
    }

    pub async fn spend_leaderboard(
        &self,
        filter: SpendLeaderboardFilter,
    ) -> Result<SpendLeaderboardReport, RepositoryError> {
        if !(1..=100).contains(&filter.limit) || !filter.period.is_valid_start(filter.period_start)
        {
            return Err(RepositoryError::Validation);
        }

        let period = filter.period.as_str();
        let snapshot = sqlx::query_as::<_, SpendLeaderboardPeriodRow>(
            "SELECT period_end, refreshed_at, total_cost_amount
             FROM spend_leaderboard_periods
             WHERE period = $1
               AND period_start = $2",
        )
        .bind(period)
        .bind(filter.period_start)
        .fetch_optional(&self.pool)
        .await?;

        let rows = sqlx::query_as::<_, SpendLeaderboardRow>(
            "SELECT entry.rank,
                    entry.user_id,
                    account_user.display_name,
                    entry.request_count,
                    entry.priced_request_count,
                    entry.total_tokens,
                    entry.cost_amount
             FROM spend_leaderboard_entries AS entry
             JOIN users AS account_user ON account_user.id = entry.user_id
             WHERE entry.period = $1
               AND entry.period_start = $2
             ORDER BY entry.rank
             LIMIT $3",
        )
        .bind(period)
        .bind(filter.period_start)
        .bind(filter.limit)
        .fetch_all(&self.pool)
        .await?;

        let previous_period_start = sqlx::query_scalar::<_, Option<NaiveDate>>(
            "SELECT max(period_start)
             FROM spend_leaderboard_periods
             WHERE period = $1
               AND period_start < $2",
        )
        .bind(period)
        .bind(filter.period_start)
        .fetch_one(&self.pool)
        .await?;
        let next_period_start = sqlx::query_scalar::<_, Option<NaiveDate>>(
            "SELECT min(period_start)
             FROM spend_leaderboard_periods
             WHERE period = $1
               AND period_start > $2",
        )
        .bind(period)
        .bind(filter.period_start)
        .fetch_one(&self.pool)
        .await?;

        let period_end = snapshot.as_ref().map_or_else(
            || filter.period.end_after(filter.period_start),
            |row| row.period_end,
        );
        let entries = rows
            .into_iter()
            .map(|row| SpendLeaderboardEntry {
                rank: row.rank,
                user_id: row.user_id,
                display_name: row.display_name,
                request_count: row.request_count,
                priced_request_count: row.priced_request_count,
                total_tokens: row.total_tokens,
                cost_amount: row.cost_amount,
            })
            .collect();

        Ok(SpendLeaderboardReport {
            period: period.into(),
            period_start: filter.period_start,
            period_end,
            refreshed_at: snapshot.as_ref().map(|row| row.refreshed_at),
            total_cost_amount: snapshot
                .as_ref()
                .map_or(rust_decimal::Decimal::ZERO, |row| row.total_cost_amount),
            previous_period_start,
            next_period_start,
            entries,
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
        let result = self
            .insert_batch(std::slice::from_ref(event))
            .await?
            .into_iter()
            .next()
            .expect("one input event produces one batch result");
        match result.outcome {
            RequestLogBatchInsertOutcome::Inserted => Ok(RequestLogInsertOutcome::Inserted),
            RequestLogBatchInsertOutcome::ExactDuplicate => {
                Ok(RequestLogInsertOutcome::ExactDuplicate)
            }
            RequestLogBatchInsertOutcome::DuplicateConflict => {
                Err(RepositoryError::DuplicateConflict { id: event.id })
            }
            RequestLogBatchInsertOutcome::InvalidResponseStatus { status } => {
                Err(RepositoryError::InvalidResponseStatus { status })
            }
        }
    }

    /// Inserts a bounded set of terminal events with one multi-row statement.
    ///
    /// Per-event validation and duplicate classification remain isolated so one
    /// malformed status or conflicting duplicate does not hide valid peers.
    /// Database-level failures still fail the whole statement transactionally;
    /// the worker falls back to single-event insertion on that exceptional path.
    pub async fn insert_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<RequestLogBatchInsertResult>, RepositoryError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        let mut outcomes = vec![None; events.len()];
        let mut valid = Vec::with_capacity(events.len());
        let mut first_valid_index = HashMap::<Uuid, usize>::with_capacity(events.len());
        for (index, event) in events.iter().enumerate() {
            let status = match event
                .response_status_code
                .map(validate_response_status)
                .transpose()
            {
                Ok(status) => status,
                Err(RepositoryError::InvalidResponseStatus { status }) => {
                    outcomes[index] =
                        Some(RequestLogBatchInsertOutcome::InvalidResponseStatus { status });
                    continue;
                }
                Err(error) => return Err(error),
            };
            first_valid_index.entry(event.id).or_insert(index);
            valid.push((index, status));
        }

        if !valid.is_empty() {
            let mut batch = RequestLogInsertBatch::with_capacity(valid.len());
            for (index, status) in &valid {
                batch.push(&events[*index], *status);
            }
            let inserted_ids = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO request_logs \
                 (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,\
                  request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,\
                  channel_id,outcome,response_status_code,streamed,ttft_ms,total_duration_ms,\
                  output_tokens_per_second,input_tokens,cached_input_tokens,cache_write_tokens,\
                  output_tokens,model_id,currency,price_unit_tokens,price_effective_at,\
                  input_unit_price,cached_input_unit_price,cache_write_unit_price,\
                  output_unit_price,cost_amount,error_code,error_summary,reasoning_tokens,\
                  reasoning_effort,fast_mode) \
                 SELECT input.id,input.started_at,input.completed_at,input.user_id,input.api_key_id,\
                        input.request_source,input.api_format::api_format,input.request_protocol,\
                        input.client_model,input.upstream_model,input.model_rule_id,\
                        input.channel_group_id,input.channel_id,input.outcome,\
                        input.response_status_code,input.streamed,input.ttft_ms,\
                        input.total_duration_ms,input.output_tokens_per_second,input.input_tokens,\
                        input.cached_input_tokens,input.cache_write_tokens,input.output_tokens,\
                        input.model_id,input.currency::char(3),input.price_unit_tokens,\
                        input.price_effective_at,input.input_unit_price,\
                        input.cached_input_unit_price,input.cache_write_unit_price,\
                        input.output_unit_price,input.cost_amount,input.error_code,\
                        input.error_summary,input.reasoning_tokens,input.reasoning_effort,\
                        input.fast_mode \
                 FROM UNNEST(\
                    $1::uuid[],$2::timestamptz[],$3::timestamptz[],$4::uuid[],$5::uuid[],\
                    $6::text[],$7::text[],$8::text[],$9::text[],$10::text[],$11::uuid[],\
                    $12::uuid[],$13::uuid[],$14::text[],$15::int2[],$16::bool[],$17::int4[],\
                    $18::int4[],$19::numeric[],$20::int8[],$21::int8[],$22::int8[],$23::int8[],\
                    $24::uuid[],$25::text[],$26::int8[],$27::timestamptz[],$28::numeric[],\
                    $29::numeric[],$30::numeric[],$31::numeric[],$32::numeric[],$33::text[],\
                    $34::text[],$35::int8[],$36::text[],$37::bool[]\
                 ) AS input(\
                    id,started_at,completed_at,user_id,api_key_id,request_source,api_format,\
                    request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,\
                    channel_id,outcome,response_status_code,streamed,ttft_ms,total_duration_ms,\
                    output_tokens_per_second,input_tokens,cached_input_tokens,cache_write_tokens,\
                    output_tokens,model_id,currency,price_unit_tokens,price_effective_at,\
                    input_unit_price,cached_input_unit_price,cache_write_unit_price,\
                    output_unit_price,cost_amount,error_code,error_summary,reasoning_tokens,\
                    reasoning_effort,fast_mode\
                 ) \
                 ON CONFLICT (id) DO NOTHING \
                 RETURNING id",
            )
                .bind(&batch.ids)
                .bind(&batch.started_at)
                .bind(&batch.completed_at)
                .bind(&batch.user_ids)
                .bind(&batch.api_key_ids)
                .bind(&batch.request_sources)
                .bind(&batch.api_formats)
                .bind(&batch.request_protocols)
                .bind(&batch.client_models)
                .bind(&batch.upstream_models)
                .bind(&batch.model_rule_ids)
                .bind(&batch.channel_group_ids)
                .bind(&batch.channel_ids)
                .bind(&batch.outcomes)
                .bind(&batch.response_status_codes)
                .bind(&batch.streamed)
                .bind(&batch.ttft_ms)
                .bind(&batch.total_duration_ms)
                .bind(&batch.output_tokens_per_second)
                .bind(&batch.input_tokens)
                .bind(&batch.cached_input_tokens)
                .bind(&batch.cache_write_tokens)
                .bind(&batch.output_tokens)
                .bind(&batch.model_ids)
                .bind(&batch.currencies)
                .bind(&batch.price_unit_tokens)
                .bind(&batch.price_effective_at)
                .bind(&batch.input_unit_price)
                .bind(&batch.cached_input_unit_price)
                .bind(&batch.cache_write_unit_price)
                .bind(&batch.output_unit_price)
                .bind(&batch.cost_amount)
                .bind(&batch.error_codes)
                .bind(&batch.error_summaries)
                .bind(&batch.reasoning_tokens)
                .bind(&batch.reasoning_efforts)
                .bind(&batch.fast_modes)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .collect::<HashSet<_>>();

            let mut needs_existing = HashSet::new();
            for (index, _) in &valid {
                let event = &events[*index];
                if inserted_ids.contains(&event.id)
                    && first_valid_index.get(&event.id) == Some(index)
                {
                    outcomes[*index] = Some(RequestLogBatchInsertOutcome::Inserted);
                } else {
                    needs_existing.insert(event.id);
                }
            }

            if !needs_existing.is_empty() {
                let ids = needs_existing.iter().copied().collect::<Vec<_>>();
                let existing = sqlx::query_as::<_, StoredRequestLog>(
                    "SELECT id,started_at,completed_at,user_id,api_key_id,request_source,\
                            api_format::text AS api_format,request_protocol,client_model,\
                            upstream_model,model_rule_id,channel_group_id,channel_id,outcome,\
                            response_status_code,streamed,ttft_ms,total_duration_ms,\
                            output_tokens_per_second,input_tokens,cached_input_tokens,\
                            cache_write_tokens,output_tokens,reasoning_tokens,\
                            model_id,currency,price_unit_tokens,price_effective_at,input_unit_price,\
                            cached_input_unit_price,cache_write_unit_price,output_unit_price,\
                            cost_amount,error_code,error_summary,reasoning_effort,fast_mode \
                     FROM request_logs WHERE id = ANY($1)",
                )
                .bind(&ids)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|row| (row.id, row))
                .collect::<HashMap<_, _>>();

                for (index, status) in &valid {
                    if outcomes[*index].is_some() {
                        continue;
                    }
                    let event = &events[*index];
                    let stored = existing
                        .get(&event.id)
                        .ok_or(RepositoryError::DuplicateDisappeared { id: event.id })?;
                    outcomes[*index] = Some(
                        if stored.matches(
                            event,
                            normalize_timestamp(event.started_at),
                            normalize_timestamp(event.completed_at),
                            *status,
                        ) {
                            RequestLogBatchInsertOutcome::ExactDuplicate
                        } else {
                            RequestLogBatchInsertOutcome::DuplicateConflict
                        },
                    );
                }
            }
        }

        Ok(events
            .iter()
            .zip(outcomes)
            .map(|(event, outcome)| RequestLogBatchInsertResult {
                request_log_id: event.id,
                outcome: outcome.expect("every batch input receives an outcome"),
            })
            .collect())
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
        Ok(self
            .settle_batch(&[request_log_id])
            .await?
            .into_iter()
            .next()
            .expect("one request-log id produces one settlement outcome")
            .1)
    }

    /// Claims and applies a set of billable terminal logs with batched account
    /// updates in one transaction.
    ///
    /// Costs are aggregated per user and API key before those account rows are
    /// updated. The returned vector is deduplicated by request-log id while
    /// preserving first-seen input order.
    pub async fn settle_batch(
        &self,
        request_log_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, RequestLogSettlementOutcome)>, RepositoryError> {
        let mut seen = HashSet::with_capacity(request_log_ids.len());
        let request_log_ids = request_log_ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        if request_log_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut transaction = self.pool.begin().await?;
        let claimed = sqlx::query_as::<_, ClaimedRequestLog>(
            "UPDATE request_logs AS log
             SET billed_at = now()
             FROM api_keys AS key
             WHERE log.id = ANY($1)
               AND log.billed_at IS NULL
               AND log.cost_amount IS NOT NULL
               AND key.id = log.api_key_id
               AND key.user_id = log.user_id
             RETURNING log.id, log.user_id, log.api_key_id, log.cost_amount",
        )
        .bind(&request_log_ids)
        .fetch_all(&mut *transaction)
        .await?;

        let claimed_ids = claimed.iter().map(|row| row.id).collect::<HashSet<_>>();
        let unclaimed_ids = request_log_ids
            .iter()
            .copied()
            .filter(|id| !claimed_ids.contains(id))
            .collect::<Vec<_>>();
        let eligibility = if unclaimed_ids.is_empty() {
            HashMap::new()
        } else {
            sqlx::query_as::<_, SettlementEligibility>(
                "SELECT log.id,
                        log.billed_at,
                        log.cost_amount,
                        key.user_id AS api_key_user_id,
                        log.user_id
                 FROM request_logs AS log
                 LEFT JOIN api_keys AS key ON key.id = log.api_key_id
                 WHERE log.id = ANY($1)",
            )
            .bind(&unclaimed_ids)
            .fetch_all(&mut *transaction)
            .await?
            .into_iter()
            .map(|row| (row.id, row))
            .collect::<HashMap<_, _>>()
        };

        let mut quota_by_api_key = HashMap::new();
        if !claimed.is_empty() {
            let mut user_costs = BTreeMap::<Uuid, rust_decimal::Decimal>::new();
            let mut api_key_costs = BTreeMap::<(Uuid, Uuid), rust_decimal::Decimal>::new();
            for row in &claimed {
                *user_costs.entry(row.user_id).or_default() += row.cost_amount;
                *api_key_costs
                    .entry((row.api_key_id, row.user_id))
                    .or_default() += row.cost_amount;
            }

            let mut users = QueryBuilder::<Postgres>::new(
                "UPDATE users AS account \
                 SET balance_amount = account.balance_amount - batch.cost_amount \
                 FROM (",
            );
            users.push_values(user_costs.iter(), |mut row, (user_id, cost_amount)| {
                row.push_bind(*user_id).push_bind(*cost_amount);
            });
            users.push(
                ") AS batch(user_id,cost_amount) \
                 WHERE account.id = batch.user_id \
                 RETURNING account.id",
            );
            let updated_users = users
                .build_query_scalar::<Uuid>()
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .collect::<HashSet<_>>();
            if updated_users.len() != user_costs.len() {
                return Err(RepositoryError::SettlementClaimInvalidated { id: claimed[0].id });
            }

            let mut api_keys = QueryBuilder::<Postgres>::new(
                "UPDATE api_keys AS key \
                 SET quota_used_amount = key.quota_used_amount + batch.cost_amount \
                 FROM (",
            );
            api_keys.push_values(
                api_key_costs.iter(),
                |mut row, ((api_key_id, user_id), cost_amount)| {
                    row.push_bind(*api_key_id)
                        .push_bind(*user_id)
                        .push_bind(*cost_amount);
                },
            );
            api_keys.push(
                ") AS batch(api_key_id,user_id,cost_amount) \
                 WHERE key.id = batch.api_key_id \
                   AND key.user_id = batch.user_id \
                 RETURNING key.id,key.quota_used_amount",
            );
            quota_by_api_key = api_keys
                .build_query_as::<UpdatedApiKey>()
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .map(|row| (row.id, row.quota_used_amount))
                .collect();
            if quota_by_api_key.len() != api_key_costs.len() {
                return Err(RepositoryError::SettlementClaimInvalidated { id: claimed[0].id });
            }
        }

        let claimed = claimed
            .into_iter()
            .map(|row| {
                let quota_used_amount = quota_by_api_key
                    .get(&row.api_key_id)
                    .copied()
                    .ok_or(RepositoryError::SettlementClaimInvalidated { id: row.id })?;
                Ok((
                    row.id,
                    RequestLogSettlementOutcome::Settled {
                        request_log_id: row.id,
                        api_key_id: row.api_key_id,
                        quota_used_amount,
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, RepositoryError>>()?;
        let outcomes = request_log_ids
            .iter()
            .map(|id| {
                let outcome = claimed.get(id).cloned().unwrap_or_else(|| {
                    eligibility.get(id).map_or(
                        RequestLogSettlementOutcome::NotFound,
                        settlement_outcome_from_eligibility,
                    )
                });
                (*id, outcome)
            })
            .collect();
        transaction.commit().await?;
        Ok(outcomes)
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
        // Recovery is best-effort and must not monopolize the settlement stage
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
        Ok(self
            .settle_batch(&request_log_ids)
            .await?
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect())
    }
}

fn redact_self_service_request_log(log: &mut ConsoleRequestLog) {
    log.user_name = None;
    log.channel_id = None;
    log.channel_name = None;
}

const CONSOLE_REQUEST_LOG_COLUMNS: &str = "log.id,log.started_at,log.completed_at,log.user_id,request_user.display_name AS user_name,log.api_key_id,log.request_source,log.api_format::text AS api_format,log.request_protocol,log.client_model,log.reasoning_effort,log.fast_mode,log.upstream_model,log.model_rule_id,log.channel_group_id,channel_group.name AS channel_group_name,log.channel_id,channel.name AS channel_name,log.outcome,log.response_status_code,log.streamed,log.ttft_ms,log.total_duration_ms,log.output_tokens_per_second,log.input_tokens,log.cached_input_tokens,log.cache_write_tokens,log.output_tokens,log.reasoning_tokens,log.cost_amount,log.error_code,log.error_summary,log.billed_at";

async fn query_console_request_log(
    pool: &PgPool,
    id: Uuid,
    owner_user_id: Option<Uuid>,
) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
    let mut query = QueryBuilder::<Postgres>::new(format!(
        "SELECT {CONSOLE_REQUEST_LOG_COLUMNS}
         FROM request_logs AS log
         JOIN users AS request_user ON request_user.id = log.user_id
         LEFT JOIN channel_groups AS channel_group ON channel_group.id = log.channel_group_id
         LEFT JOIN channels AS channel ON channel.id = log.channel_id
         WHERE log.id = "
    ));
    query.push_bind(id);
    if let Some(user_id) = owner_user_id {
        query.push(" AND log.user_id = ").push_bind(user_id);
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
        "SELECT {CONSOLE_REQUEST_LOG_COLUMNS}
         FROM request_logs AS log
         JOIN users AS request_user ON request_user.id = log.user_id
         LEFT JOIN channel_groups AS channel_group ON channel_group.id = log.channel_group_id
         LEFT JOIN channels AS channel ON channel.id = log.channel_id
         WHERE TRUE"
    ));
    if let Some(user_id) = owner_user_id {
        query.push(" AND log.user_id = ").push_bind(user_id);
    }
    if let Some(user_id) = filter.user_id {
        query.push(" AND log.user_id = ").push_bind(user_id);
    }
    if let Some(api_key_id) = filter.api_key_id {
        query.push(" AND log.api_key_id = ").push_bind(api_key_id);
    }
    if let Some(model) = filter.model {
        query
            .push(" AND (log.client_model = ")
            .push_bind(model.clone())
            .push(" OR log.upstream_model = ")
            .push_bind(model)
            .push(")");
    }
    if let Some(api_format) = filter.api_format {
        query
            .push(" AND log.api_format::text = ")
            .push_bind(api_format);
    }
    if let Some(outcome) = filter.outcome {
        query.push(" AND log.outcome = ").push_bind(outcome);
    }
    if let Some(started_after) = filter.started_after {
        query
            .push(" AND log.started_at >= ")
            .push_bind(started_after);
    }
    if let Some(started_before) = filter.started_before {
        query
            .push(" AND log.started_at <= ")
            .push_bind(started_before);
    }
    if let Some(billed) = filter.billed {
        if billed {
            query.push(" AND log.billed_at IS NOT NULL");
        } else {
            query.push(" AND log.billed_at IS NULL");
        }
    }
    query
        .push(" ORDER BY log.started_at DESC, log.id DESC LIMIT ")
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
struct PersonalUsageDayRow {
    date: NaiveDate,
    request_count: i64,
}

#[derive(FromRow)]
struct CostSummaryRow {
    request_count: i64,
    priced_request_count: i64,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct SpendLeaderboardRow {
    rank: i64,
    user_id: Uuid,
    display_name: String,
    request_count: i64,
    priced_request_count: i64,
    total_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct SpendLeaderboardPeriodRow {
    period_end: NaiveDate,
    refreshed_at: DateTime<Utc>,
    total_cost_amount: rust_decimal::Decimal,
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
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct CostChannelMetricRow {
    id: Uuid,
    channel_group_id: Uuid,
    channel_group_name: String,
    name: String,
    api_format: String,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

impl CostChannelMetricRow {
    fn into_metric(self) -> CostStatisticsChannel {
        CostStatisticsChannel {
            id: self.id,
            channel_group_id: self.channel_group_id,
            channel_group_name: self.channel_group_name,
            name: self.name,
            api_format: self.api_format,
            request_count: self.request_count,
            total_tokens: self.total_tokens,
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_write_tokens: self.cache_write_tokens,
            output_tokens: self.output_tokens,
            success_rate: success_rate(self.success_rate_request_count, self.succeeded_count),
            cost_amount: self.cost_amount,
        }
    }
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
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: rust_decimal::Decimal,
}

fn fold_personal_usage(
    rows: Vec<PersonalUsageDayRow>,
    started_on: NaiveDate,
    ended_on: NaiveDate,
) -> PersonalUsageReport {
    let counts = rows
        .into_iter()
        .map(|row| (row.date, row.request_count))
        .collect::<BTreeMap<_, _>>();
    let mut total_request_count = 0_i64;
    let mut active_day_count = 0_i64;
    let mut current_streak_days = 0_i64;
    let mut longest_streak_days = 0_i64;
    let mut days = Vec::with_capacity(365);
    let mut date = started_on;

    loop {
        let request_count = counts.get(&date).copied().unwrap_or_default();
        total_request_count = total_request_count.saturating_add(request_count);
        if request_count > 0 {
            active_day_count = active_day_count.saturating_add(1);
            current_streak_days = current_streak_days.saturating_add(1);
            longest_streak_days = longest_streak_days.max(current_streak_days);
        } else {
            current_streak_days = 0;
        }
        days.push(PersonalUsageDay {
            date,
            request_count,
        });

        if date >= ended_on {
            break;
        }
        let Some(next) = date.succ_opt() else {
            break;
        };
        date = next;
    }

    PersonalUsageReport {
        started_on,
        ended_on,
        total_request_count,
        active_day_count,
        current_streak_days,
        longest_streak_days,
        days,
    }
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
        model.input_tokens = model.input_tokens.saturating_add(row.input_tokens);
        model.cached_input_tokens = model
            .cached_input_tokens
            .saturating_add(row.cached_input_tokens);
        model.cache_write_tokens = model
            .cache_write_tokens
            .saturating_add(row.cache_write_tokens);
        model.output_tokens = model.output_tokens.saturating_add(row.output_tokens);
        model.cost_amount += row.cost_amount;
    }

    models
        .into_iter()
        .map(|((api_format, model), metric)| CostStatisticsModel {
            api_format,
            model,
            request_count: metric.request_count,
            total_tokens: metric.total_tokens,
            input_tokens: metric.input_tokens,
            cached_input_tokens: metric.cached_input_tokens,
            cache_write_tokens: metric.cache_write_tokens,
            output_tokens: metric.output_tokens,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestLogBatchInsertResult {
    pub request_log_id: Uuid,
    pub outcome: RequestLogBatchInsertOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogBatchInsertOutcome {
    Inserted,
    ExactDuplicate,
    DuplicateConflict,
    InvalidResponseStatus { status: u16 },
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
pub(crate) struct RequestLogIngestRecord {
    pub sequence: i64,
    pub request_log_id: Uuid,
    pub schema_version: i16,
    pub payload: Vec<u8>,
    pub attempt_count: i32,
}

impl RequestLogIngestRecord {
    pub(crate) fn encoded(&self) -> EncodedRequestLog {
        EncodedRequestLog {
            request_log_id: self.request_log_id,
            schema_version: self.schema_version,
            payload: self.payload.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, FromRow)]
pub(crate) struct RequestLogIngestBacklog {
    pub row_count: i64,
    pub oldest_staged_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, FromRow)]
pub(crate) struct RequestLogSettlementBacklog {
    pub row_count: i64,
    pub oldest_completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestLogPoolStatus {
    pub size: u32,
    pub idle: usize,
}

#[derive(FromRow)]
struct ClaimedRequestLog {
    id: Uuid,
    user_id: Uuid,
    api_key_id: Uuid,
    cost_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct UpdatedApiKey {
    id: Uuid,
    quota_used_amount: rust_decimal::Decimal,
}

#[derive(FromRow)]
struct SettlementEligibility {
    id: Uuid,
    billed_at: Option<DateTime<Utc>>,
    cost_amount: Option<rust_decimal::Decimal>,
    api_key_user_id: Option<Uuid>,
    user_id: Uuid,
}

fn settlement_outcome_from_eligibility(
    eligibility: &SettlementEligibility,
) -> RequestLogSettlementOutcome {
    if eligibility.billed_at.is_some() {
        return RequestLogSettlementOutcome::AlreadyBilled;
    }
    if eligibility.cost_amount.is_none() {
        return RequestLogSettlementOutcome::NotBillable;
    }
    if eligibility.api_key_user_id != Some(eligibility.user_id) {
        return RequestLogSettlementOutcome::AccountMismatch;
    }
    // A concurrent claimer either commits and is observed as `AlreadyBilled`,
    // or rolls back and leaves a later recovery pass to claim the row.
    RequestLogSettlementOutcome::NotBillable
}

struct RequestLogInsertBatch {
    ids: Vec<Uuid>,
    started_at: Vec<DateTime<Utc>>,
    completed_at: Vec<DateTime<Utc>>,
    user_ids: Vec<Uuid>,
    api_key_ids: Vec<Uuid>,
    request_sources: Vec<String>,
    api_formats: Vec<String>,
    request_protocols: Vec<String>,
    client_models: Vec<String>,
    upstream_models: Vec<Option<String>>,
    model_rule_ids: Vec<Option<Uuid>>,
    channel_group_ids: Vec<Option<Uuid>>,
    channel_ids: Vec<Option<Uuid>>,
    outcomes: Vec<String>,
    response_status_codes: Vec<Option<i16>>,
    streamed: Vec<bool>,
    ttft_ms: Vec<Option<i32>>,
    total_duration_ms: Vec<i32>,
    output_tokens_per_second: Vec<Option<rust_decimal::Decimal>>,
    input_tokens: Vec<Option<i64>>,
    cached_input_tokens: Vec<Option<i64>>,
    cache_write_tokens: Vec<Option<i64>>,
    output_tokens: Vec<Option<i64>>,
    reasoning_tokens: Vec<Option<i64>>,
    model_ids: Vec<Option<Uuid>>,
    currencies: Vec<Option<String>>,
    price_unit_tokens: Vec<Option<i64>>,
    price_effective_at: Vec<Option<DateTime<Utc>>>,
    input_unit_price: Vec<Option<rust_decimal::Decimal>>,
    cached_input_unit_price: Vec<Option<rust_decimal::Decimal>>,
    cache_write_unit_price: Vec<Option<rust_decimal::Decimal>>,
    output_unit_price: Vec<Option<rust_decimal::Decimal>>,
    cost_amount: Vec<Option<rust_decimal::Decimal>>,
    error_codes: Vec<Option<String>>,
    error_summaries: Vec<Option<String>>,
    reasoning_efforts: Vec<Option<String>>,
    fast_modes: Vec<bool>,
}

impl RequestLogInsertBatch {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            ids: Vec::with_capacity(capacity),
            started_at: Vec::with_capacity(capacity),
            completed_at: Vec::with_capacity(capacity),
            user_ids: Vec::with_capacity(capacity),
            api_key_ids: Vec::with_capacity(capacity),
            request_sources: Vec::with_capacity(capacity),
            api_formats: Vec::with_capacity(capacity),
            request_protocols: Vec::with_capacity(capacity),
            client_models: Vec::with_capacity(capacity),
            upstream_models: Vec::with_capacity(capacity),
            model_rule_ids: Vec::with_capacity(capacity),
            channel_group_ids: Vec::with_capacity(capacity),
            channel_ids: Vec::with_capacity(capacity),
            outcomes: Vec::with_capacity(capacity),
            response_status_codes: Vec::with_capacity(capacity),
            streamed: Vec::with_capacity(capacity),
            ttft_ms: Vec::with_capacity(capacity),
            total_duration_ms: Vec::with_capacity(capacity),
            output_tokens_per_second: Vec::with_capacity(capacity),
            input_tokens: Vec::with_capacity(capacity),
            cached_input_tokens: Vec::with_capacity(capacity),
            cache_write_tokens: Vec::with_capacity(capacity),
            output_tokens: Vec::with_capacity(capacity),
            reasoning_tokens: Vec::with_capacity(capacity),
            model_ids: Vec::with_capacity(capacity),
            currencies: Vec::with_capacity(capacity),
            price_unit_tokens: Vec::with_capacity(capacity),
            price_effective_at: Vec::with_capacity(capacity),
            input_unit_price: Vec::with_capacity(capacity),
            cached_input_unit_price: Vec::with_capacity(capacity),
            cache_write_unit_price: Vec::with_capacity(capacity),
            output_unit_price: Vec::with_capacity(capacity),
            cost_amount: Vec::with_capacity(capacity),
            error_codes: Vec::with_capacity(capacity),
            error_summaries: Vec::with_capacity(capacity),
            reasoning_efforts: Vec::with_capacity(capacity),
            fast_modes: Vec::with_capacity(capacity),
        }
    }

    fn push(&mut self, event: &RequestLogEvent, response_status_code: Option<i16>) {
        let billing = event.billing.as_ref();
        let usage = billing.and_then(|billing| billing.usage.as_ref());
        let price = billing.map(|billing| &billing.price);
        self.ids.push(event.id);
        self.started_at.push(normalize_timestamp(event.started_at));
        self.completed_at
            .push(normalize_timestamp(event.completed_at));
        self.user_ids.push(event.user_id);
        self.api_key_ids.push(event.api_key_id);
        self.request_sources
            .push(event.request_source.as_str().into());
        self.api_formats.push(api_format_name(event).into());
        self.request_protocols
            .push(event.request_protocol.as_str().into());
        self.client_models.push(event.client_model.clone());
        self.upstream_models.push(event.upstream_model.clone());
        self.model_rule_ids.push(event.model_rule_id);
        self.channel_group_ids.push(event.channel_group_id);
        self.channel_ids.push(event.channel_id);
        self.outcomes.push(event.outcome.as_str().into());
        self.response_status_codes.push(response_status_code);
        self.streamed.push(event.streamed);
        self.ttft_ms.push(event.ttft_ms);
        self.total_duration_ms.push(event.total_duration_ms);
        self.output_tokens_per_second
            .push(billing.and_then(|billing| billing.output_tokens_per_second));
        self.input_tokens
            .push(usage.map(|usage| usage.input_tokens));
        self.cached_input_tokens
            .push(usage.map(|usage| usage.cached_input_tokens));
        self.cache_write_tokens
            .push(usage.map(|usage| usage.cache_write_tokens));
        self.output_tokens
            .push(usage.map(|usage| usage.output_tokens));
        self.reasoning_tokens
            .push(usage.map(|usage| usage.reasoning_tokens));
        self.model_ids.push(event.model_id);
        self.currencies
            .push(price.map(|price| price.currency.clone()));
        self.price_unit_tokens
            .push(price.map(|price| price.price_unit_tokens));
        self.price_effective_at
            .push(price.map(|price| normalize_timestamp(price.price_effective_at)));
        self.input_unit_price
            .push(price.map(|price| price.input_unit_price));
        self.cached_input_unit_price
            .push(price.map(|price| price.cached_input_unit_price));
        self.cache_write_unit_price
            .push(price.map(|price| price.cache_write_unit_price));
        self.output_unit_price
            .push(price.map(|price| price.output_unit_price));
        self.cost_amount
            .push(billing.and_then(|billing| billing.cost_amount));
        self.error_codes.push(event.error_code.clone());
        self.error_summaries.push(event.error_summary.clone());
        self.reasoning_efforts.push(event.reasoning_effort.clone());
        self.fast_modes.push(event.fast_mode);
    }
}

fn append_copy_bytea(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(b"\\\\x");
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in value {
        output.push(HEX[(byte >> 4) as usize]);
        output.push(HEX[(byte & 0x0f) as usize]);
    }
}

#[derive(FromRow)]
struct StoredRequestLog {
    id: Uuid,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    user_id: Uuid,
    api_key_id: Uuid,
    request_source: String,
    api_format: String,
    request_protocol: String,
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
    reasoning_tokens: Option<i64>,
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
    error_summary: Option<String>,
    reasoning_effort: Option<String>,
    fast_mode: bool,
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
            && self.request_source == event.request_source.as_str()
            && self.api_format == api_format_name(event)
            && self.request_protocol == event.request_protocol.as_str()
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
            && self.reasoning_tokens
                == event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.usage.as_ref().map(|usage| usage.reasoning_tokens))
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
            && self.error_code == event.error_code
            && self.error_summary == event.error_summary
            && self.reasoning_effort == event.reasoning_effort
            && self.fast_mode == event.fast_mode
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

    /// Ensures the hidden, administrator-owned identity used solely by
    /// periodic upstream test request logs. Its API key secret is generated at
    /// first startup and never returned through a management API.
    pub async fn ensure_system_probe_identity(
        &self,
    ) -> Result<SystemProbeIdentity, RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        let user_created = sqlx::query_scalar::<_, bool>(
            "INSERT INTO users
             (id,email,display_name,role,status,balance_amount,user_group_id,is_system)
             VALUES ($1,NULL,$2,'admin','active',0,$3,true)
             ON CONFLICT DO NOTHING
             RETURNING true",
        )
        .bind(SYSTEM_PROBE_USER_ID)
        .bind(SYSTEM_PROBE_DISPLAY_NAME)
        .bind(DEFAULT_ADMIN_GROUP_ID)
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);
        let key_created = sqlx::query_scalar::<_, bool>(
            "INSERT INTO api_keys
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
              allowed_group_ids,allowed_channel_ids,is_system)
             VALUES (
                 $1,$2,$3,$4,'active',
                 ARRAY['open_ai_chat_completions','open_ai_responses']::api_format[],
                 ARRAY['proxy','models.read']::text[],
                 ARRAY[]::uuid[],ARRAY[]::uuid[],true
             )
             ON CONFLICT DO NOTHING
             RETURNING true",
        )
        .bind(SYSTEM_PROBE_API_KEY_ID)
        .bind(SYSTEM_PROBE_USER_ID)
        .bind(SYSTEM_PROBE_API_KEY_NAME)
        .bind(generate_api_key_secret())
        .fetch_optional(&mut *transaction)
        .await?
        .unwrap_or(false);

        let valid = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                 SELECT 1
                 FROM api_keys AS key
                 JOIN users AS user_account ON user_account.id=key.user_id
                 WHERE key.id=$1
                   AND key.user_id=$2
                   AND key.is_system
                   AND user_account.is_system
                   AND user_account.status='active'
                   AND user_account.role='admin'
             )",
        )
        .bind(SYSTEM_PROBE_API_KEY_ID)
        .bind(SYSTEM_PROBE_USER_ID)
        .fetch_one(&mut *transaction)
        .await?;
        if !valid {
            return Err(RepositoryError::Validation);
        }
        if user_created || key_created {
            sqlx::query(
                "INSERT INTO audit_logs
                 (id,actor_type,action,object_type,object_id,before_redacted,after_redacted)
                 VALUES ($1,'system','initialize','system_probe_identity',$2,'{}',$3)",
            )
            .bind(Uuid::new_v4())
            .bind(SYSTEM_PROBE_API_KEY_ID)
            .bind(json!({
                "user_id": SYSTEM_PROBE_USER_ID,
                "api_key_id": SYSTEM_PROBE_API_KEY_ID,
            }))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(SystemProbeIdentity {
            user_id: SYSTEM_PROBE_USER_ID,
            api_key_id: SYSTEM_PROBE_API_KEY_ID,
        })
    }

    /// Inserts the first database-backed forwarding policy from bootstrap TOML.
    ///
    /// Existing rows are never overwritten, so all later runtime reads use the
    /// database as the sole source of truth.
    pub async fn ensure_system_settings(
        &self,
        input: SystemSettingsInput,
    ) -> Result<(), RepositoryError> {
        validate_system_settings_input(&input)?;
        let value = serde_json::to_value(&input).expect("system settings serialize");
        let mut transaction = self.begin_serializable().await?;
        let inserted = sqlx::query_scalar::<_, DateTime<Utc>>(
            "INSERT INTO system_settings (setting_key,value) VALUES ($1,$2) \
             ON CONFLICT (setting_key) DO NOTHING RETURNING updated_at",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .bind(&value)
        .fetch_optional(&mut *transaction)
        .await?;
        if inserted.is_some() {
            sqlx::query(
                "INSERT INTO audit_logs \
                 (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) \
                 VALUES ($1,'system','initialize','system_settings',$2,'{}',$3)",
            )
            .bind(Uuid::new_v4())
            .bind(forwarding_settings_object_id())
            .bind(system_settings_audit_value(&value))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
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

    /// Loads every record needed to build one coherent data-plane snapshot.
    pub async fn load_runtime(&self) -> Result<RuntimeConfigRecords, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *transaction)
            .await?;
        let records = Self::load_runtime_transaction(&mut transaction).await?;
        transaction.commit().await?;
        Ok(records)
    }

    pub async fn load_runtime_transaction(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<RuntimeConfigRecords, RepositoryError> {
        Ok(RuntimeConfigRecords {
            control_plane: Self::load_transaction(transaction).await?,
            system_settings: Self::load_system_settings_transaction(transaction).await?,
        })
    }

    async fn load_system_settings_transaction(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<SystemSettingsRecord, RepositoryError> {
        sqlx::query_as::<_, SystemSettingsRecord>(
            "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=$1",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)
    }

    pub async fn system_settings(&self) -> Result<SystemSettingsView, RepositoryError> {
        let record = sqlx::query_as::<_, SystemSettingsRecord>(
            "SELECT setting_key,value,updated_at FROM system_settings WHERE setting_key=$1",
        )
        .bind(FORWARDING_SETTINGS_KEY)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        system_settings_view(record)
    }

    pub async fn user_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<UserSettingsView>, RepositoryError> {
        sqlx::query_as::<_, UserSettingsView>(
            "SELECT websocket_enabled,updated_at
             FROM users
             WHERE id=$1 AND status='active' AND deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn update_user_settings(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        input: UserSettingsInput,
    ) -> Result<Option<UserSettingsView>, RepositoryError> {
        sqlx::query_as::<_, UserSettingsView>(
            "UPDATE users
             SET websocket_enabled=$2
             WHERE id=$1 AND status='active' AND deleted_at IS NULL
             RETURNING websocket_enabled,updated_at",
        )
        .bind(user_id)
        .bind(input.websocket_enabled)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn load_transaction(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<ControlPlaneRecords, RepositoryError> {
        let api_keys = sqlx::query_as::<_, ApiKeyRecord>("SELECT k.id, k.user_id, u.status AS user_status, u.websocket_enabled AS user_websocket_enabled, k.secret_value, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.allowed_channel_ids, k.requests_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount FROM api_keys k JOIN users u ON u.id = k.user_id WHERE NOT k.is_system ORDER BY k.id").fetch_all(&mut **transaction).await?;
        let models = sqlx::query_as::<_, ModelRecord>("SELECT id,source_model_id,currency,price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,advanced_billing FROM models ORDER BY id").fetch_all(&mut **transaction).await?;
        let model_rules = sqlx::query_as::<_, ModelRuleRecord>("SELECT r.id, r.client_model, r.api_format::text AS api_format, r.upstream_model_id, m.enabled AS upstream_model_enabled, m.currency AS upstream_model_currency, m.price_unit_tokens, m.price_effective_at, m.input_unit_price, m.cached_input_unit_price, m.cache_write_unit_price, m.output_unit_price, m.advanced_billing, m.source_model_id AS upstream_model, r.channel_group_ids, r.channel_ids, r.enabled FROM model_rules r JOIN models m ON m.id = r.upstream_model_id ORDER BY r.id").fetch_all(&mut **transaction).await?;
        let groups = sqlx::query_as::<_, ChannelGroupRecord>("SELECT id, name, api_format::text AS api_format, connector_kind, priority, selection_strategy, enabled FROM channel_groups ORDER BY id").fetch_all(&mut **transaction).await?;
        let channels = sqlx::query_as::<_, ChannelRecord>("SELECT id, channel_group_id, api_format::text AS api_format, name, base_url, enabled, supports_websocket, auto_disabled, auto_disable_allowed, weight, billing_multiplier, proxy_id, config_template_id, override_document, connect_timeout_ms, response_header_timeout_ms, stream_idle_timeout_ms, upstream_auth_kind, upstream_auth_header_name, upstream_api_key, available_models, test_model FROM channels ORDER BY id").fetch_all(&mut **transaction).await?;
        let proxies = sqlx::query_as::<_, ProxyRecord>("SELECT id, name, proxy_url, username, password, no_proxy_hosts, enabled FROM proxies ORDER BY id").fetch_all(&mut **transaction).await?;
        let templates = sqlx::query_as::<_, ConfigTemplateRecord>(
            "SELECT id, name, description, document, enabled FROM config_templates ORDER BY id",
        )
        .fetch_all(&mut **transaction)
        .await?;
        Ok(ControlPlaneRecords {
            api_keys,
            models,
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

    /// Applies an idempotent system-owned temporary disable only when the
    /// current persisted policy still matches the supplied sanitized failure
    /// trigger. The caller owns snapshot publication after a returned change.
    pub async fn automatically_disable_channel(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: Uuid,
        trigger: &AutomaticDisableTrigger,
    ) -> Result<Option<MutationResult>, RepositoryError> {
        let settings = system_settings_input_for_update(transaction).await?;
        if !automatic_disable_matches(&settings, trigger) {
            return Ok(None);
        }

        let before = channel_audit(transaction, id).await?;
        if before["enabled"].as_bool() != Some(true)
            || before["auto_disable_allowed"].as_bool() != Some(true)
            || before["auto_disabled"].as_bool() == Some(true)
        {
            return Ok(None);
        }
        let reason = automatic_disable_reason(trigger);
        let updated_at = sqlx::query_scalar(
            "UPDATE channels
             SET auto_disabled=true, auto_disabled_reason=$2
             WHERE id=$1
             RETURNING updated_at",
        )
        .bind(id)
        .bind(&reason)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        Ok(Some(MutationResult {
            id,
            object_type: "channel",
            action: "auto_disable",
            before_redacted: before,
            after_redacted: channel_audit(transaction, id).await?,
            created_secret: None,
            reason: Some(reason),
            updated_at,
            correlation_id: None,
        }))
    }

    /// Clears a temporary automatic disable after a successful scheduled
    /// upstream test when automatic recovery remains enabled in the current
    /// persisted settings. The caller owns snapshot publication after a
    /// returned change.
    pub async fn automatically_recover_channel(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        id: Uuid,
    ) -> Result<Option<MutationResult>, RepositoryError> {
        let settings = system_settings_input_for_update(transaction).await?;
        if !settings.scheduled_testing.auto_recover {
            return Ok(None);
        }

        let before = channel_audit(transaction, id).await?;
        if before["enabled"].as_bool() != Some(true)
            || before["auto_disabled"].as_bool() != Some(true)
        {
            return Ok(None);
        }
        let reason = "scheduled test succeeded".to_owned();
        let updated_at = sqlx::query_scalar(
            "UPDATE channels
             SET auto_disabled=false, auto_disabled_reason=NULL
             WHERE id=$1
             RETURNING updated_at",
        )
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        Ok(Some(MutationResult {
            id,
            object_type: "channel",
            action: "auto_recover",
            before_redacted: before,
            after_redacted: channel_audit(transaction, id).await?,
            created_secret: None,
            reason: Some(reason),
            updated_at,
            correlation_id: None,
        }))
    }

    pub async fn control_plane_lists(&self) -> Result<ControlPlaneLists, RepositoryError> {
        let users = sqlx::query_as::<_, ControlPlaneUser>(
            "SELECT u.id,u.email,u.display_name,u.role,u.status, \
                    (u.password_hash IS NULL AND u.email IS NOT NULL AND u.status IN ('invited','suspended','disabled')) AS can_reissue_invitation, \
                    u.user_group_id,u.default_api_key_policy_id, \
                    COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) AS effective_api_key_policy_id, \
                    u.balance_amount,u.created_at,u.updated_at \
             FROM users AS u \
             JOIN user_groups AS g ON g.id=u.user_group_id \
             WHERE NOT u.is_system AND u.deleted_at IS NULL ORDER BY u.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let user_groups = sqlx::query_as::<_, ControlPlaneUserGroup>(
            "SELECT g.id,g.name,g.description,g.default_api_key_policy_id,g.system_role, \
                    count(u.id) FILTER (WHERE u.deleted_at IS NULL AND NOT u.is_system) AS member_count, \
                    g.created_at,g.updated_at \
             FROM user_groups AS g \
             LEFT JOIN users AS u ON u.user_group_id=g.id \
             GROUP BY g.id \
             ORDER BY g.system_role NULLS LAST,g.name,g.id",
        )
        .fetch_all(&self.pool)
        .await?;
        let models = sqlx::query_as::<_, ControlPlaneModel>("SELECT id,source_model_id,display_name,provider_name,enabled,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,advanced_billing,last_synced_at,created_at,updated_at FROM models ORDER BY id").fetch_all(&self.pool).await?;
        let api_keys = sqlx::query_as::<_, ControlPlaneApiKey>("SELECT k.id, k.user_id, u.status AS user_status, k.name, k.secret_value AS secret, k.status, k.expires_at, k.allowed_api_formats::text[] AS allowed_api_formats, k.permissions, k.allowed_group_ids, k.allowed_channel_ids, k.requests_per_minute, k.max_concurrent_requests, k.quota_limit_amount, k.quota_used_amount, k.updated_at FROM api_keys k JOIN users u ON u.id=k.user_id WHERE NOT k.is_system AND u.deleted_at IS NULL ORDER BY k.id").fetch_all(&self.pool).await?;
        let api_key_policies = sqlx::query_as::<_, ControlPlaneApiKeyPolicy>("SELECT id,name,allowed_group_ids,allowed_channel_ids,enabled,created_at,updated_at FROM api_key_policies ORDER BY id").fetch_all(&self.pool).await?;
        let channel_groups = sqlx::query_as::<_, ControlPlaneChannelGroup>("SELECT id,name,api_format::text AS api_format,connector_kind,priority,selection_strategy,enabled,updated_at FROM channel_groups ORDER BY id").fetch_all(&self.pool).await?;
        let channels = sqlx::query_as::<_, ControlPlaneChannelRow>("SELECT c.id,c.channel_group_id,c.api_format::text AS api_format,g.connector_kind,(g.connector_kind <> 'openai_compatible') AS provider_managed,c.name,c.base_url,CASE WHEN g.connector_kind='codex_oauth' THEN COALESCE(co.enabled,c.enabled) ELSE c.enabled END AS enabled,c.supports_websocket,c.status_statistics_enabled,c.auto_disabled,c.auto_disabled_reason,c.auto_disable_allowed,c.weight,c.billing_multiplier,c.proxy_id,c.config_template_id,c.connect_timeout_ms,c.response_header_timeout_ms,c.stream_idle_timeout_ms,c.upstream_auth_kind,c.upstream_auth_header_name,(c.upstream_api_key IS NOT NULL) AS upstream_credential_configured,c.available_models,c.test_model,c.created_at,c.updated_at FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id LEFT JOIN codex_oauth_credentials co ON co.channel_id=c.id WHERE g.connector_kind <> 'codex_oauth' OR co.deleted_at IS NULL ORDER BY c.id").fetch_all(&self.pool).await?;
        let model_rules = sqlx::query_as::<_, ControlPlaneModelRule>("SELECT r.id,r.client_model,r.api_format::text AS api_format,r.upstream_model_id,m.enabled AS upstream_model_enabled,m.source_model_id AS upstream_model,r.description,r.channel_group_ids,r.channel_ids,r.enabled,r.updated_at FROM model_rules r JOIN models m ON m.id=r.upstream_model_id ORDER BY r.id").fetch_all(&self.pool).await?;
        let proxies = sqlx::query_as::<_, ControlPlaneProxy>("SELECT id,name,regexp_replace(regexp_replace(proxy_url, '^([^:/?#]+://)[^/?#]*@', E'\\1'), '[?#].*$', '') AS proxy_url,no_proxy_hosts,enabled,(username IS NOT NULL OR password IS NOT NULL) AS credential_configured,created_at,updated_at FROM proxies ORDER BY id").fetch_all(&self.pool).await?;
        let config_templates = sqlx::query_as::<_, ControlPlaneConfigTemplate>("SELECT id,name,description,document->>'api_format' AS api_format,enabled,created_at,updated_at FROM config_templates ORDER BY id").fetch_all(&self.pool).await?;
        Ok(ControlPlaneLists {
            users,
            user_groups,
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

    pub(crate) async fn proxy_record(
        &self,
        id: Uuid,
    ) -> Result<Option<ProxyRecord>, RepositoryError> {
        sqlx::query_as::<_, ProxyRecord>(
            "SELECT id,name,proxy_url,username,password,no_proxy_hosts,enabled \
             FROM proxies WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn control_plane_channel_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneChannelDetail>, RepositoryError> {
        sqlx::query_as::<_, ControlPlaneChannelDetail>(
            "SELECT c.id,c.channel_group_id,c.api_format::text AS api_format,g.connector_kind,(g.connector_kind <> 'openai_compatible') AS provider_managed,c.name,c.base_url,CASE WHEN g.connector_kind='codex_oauth' THEN COALESCE(co.enabled,c.enabled) ELSE c.enabled END AS enabled,c.supports_websocket,c.status_statistics_enabled,c.auto_disabled,c.auto_disabled_reason,c.auto_disable_allowed,c.weight,c.billing_multiplier,c.proxy_id,c.config_template_id,c.override_document,c.connect_timeout_ms,c.response_header_timeout_ms,c.stream_idle_timeout_ms,c.upstream_auth_kind,c.upstream_auth_header_name,c.upstream_api_key,(c.upstream_api_key IS NOT NULL) AS upstream_credential_configured,c.available_models,c.test_model,c.created_at,c.updated_at FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id LEFT JOIN codex_oauth_credentials co ON co.channel_id=c.id WHERE c.id=$1 AND (g.connector_kind <> 'codex_oauth' OR co.deleted_at IS NULL)",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn control_plane_config_template_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneConfigTemplateDetail>, RepositoryError> {
        sqlx::query_as::<_, ControlPlaneConfigTemplateDetail>(
            "SELECT id,name,description,document->>'api_format' AS api_format,document,enabled,created_at,updated_at FROM config_templates WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
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
                    permissions,allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
                    quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE user_id=$1 AND NOT is_system ORDER BY created_at DESC,id DESC",
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
                    permissions,allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
                    quota_limit_amount,quota_used_amount,created_at,updated_at \
             FROM api_keys WHERE id=$1 AND user_id=$2 AND NOT is_system",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(RepositoryError::from)
    }

    pub async fn own_api_key_options(
        &self,
        user_id: Uuid,
    ) -> Result<SelfApiKeyOptions, RepositoryError> {
        let policy = sqlx::query_as::<_, SelfApiKeyPolicy>(
            "SELECT p.id,p.name,p.allowed_group_ids,p.allowed_channel_ids,p.enabled \
             FROM users AS u \
             JOIN user_groups AS g ON g.id=u.user_group_id \
             JOIN api_key_policies AS p \
               ON p.id=COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
             WHERE u.id=$1 AND u.status='active' AND u.deleted_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::DefaultApiKeyPolicyRequired)?;
        ensure_policy_enabled(&policy)?;

        let groups = sqlx::query_as::<_, SelfApiKeyGroupOption>(
            "SELECT id,name,api_format::text AS api_format,enabled \
             FROM channel_groups \
             WHERE id = ANY($1) \
             ORDER BY api_format,name,id",
        )
        .bind(&policy.allowed_group_ids)
        .fetch_all(&self.pool)
        .await?;
        let channels = sqlx::query_as::<_, SelfApiKeyChannelOption>(
            "SELECT c.id,c.channel_group_id,g.name AS channel_group_name, \
                    c.api_format::text AS api_format,c.name,c.enabled,c.auto_disabled \
             FROM channels AS c \
             JOIN channel_groups AS g ON g.id=c.channel_group_id \
             WHERE c.channel_group_id = ANY($1) OR c.id = ANY($2) \
             ORDER BY c.api_format,g.name,c.name,c.id",
        )
        .bind(&policy.allowed_group_ids)
        .bind(&policy.allowed_channel_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(SelfApiKeyOptions {
            policy_id: policy.id,
            policy_name: policy.name,
            groups,
            channels,
        })
    }

    pub async fn create_own_api_key(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        user_id: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<MutationResult, RepositoryError> {
        validate_self_api_key_input(
            &input.name,
            &input.allowed_group_ids,
            &input.allowed_channel_ids,
            input.requests_per_minute,
            input.max_concurrent_requests,
            input.quota_limit_amount,
            false,
        )?;
        let policy = load_self_api_key_policy(transaction, user_id).await?;
        let allowed_api_formats = resolve_self_api_key_targets(
            transaction,
            &input.allowed_group_ids,
            &input.allowed_channel_ids,
            &policy,
        )
        .await?;
        let id = Uuid::new_v4();
        let secret = generate_api_key_secret();
        let permissions = ["proxy", "models.read"];
        let updated_at = sqlx::query_scalar(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,expires_at,allowed_api_formats,permissions, \
              allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests, \
              quota_limit_amount) \
             VALUES ($1,$2,$3,$4,'active',$5,$6::api_format[],$7,$8,$9,$10,$11,$12) \
             RETURNING updated_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&secret)
        .bind(input.expires_at)
        .bind(&allowed_api_formats)
        .bind(permissions)
        .bind(&input.allowed_group_ids)
        .bind(&input.allowed_channel_ids)
        .bind(input.requests_per_minute)
        .bind(input.max_concurrent_requests)
        .bind(input.quota_limit_amount)
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
        if !matches!(input.status.as_str(), "active" | "disabled") {
            return Err(RepositoryError::Validation);
        }
        let current = sqlx::query_as::<_, SelfApiKeyCurrent>(
            "SELECT allowed_group_ids,allowed_channel_ids \
             FROM api_keys \
             WHERE id=$1 AND user_id=$2 FOR UPDATE",
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
        let targets_changed = !same_uuid_set(&current.allowed_group_ids, &input.allowed_group_ids)
            || !same_uuid_set(&current.allowed_channel_ids, &input.allowed_channel_ids);
        validate_self_api_key_input(
            &input.name,
            &input.allowed_group_ids,
            &input.allowed_channel_ids,
            input.requests_per_minute,
            input.max_concurrent_requests,
            input.quota_limit_amount,
            !targets_changed,
        )?;
        let allowed_api_formats = if targets_changed {
            let policy = load_self_api_key_policy(transaction, user_id).await?;
            Some(
                resolve_self_api_key_targets(
                    transaction,
                    &input.allowed_group_ids,
                    &input.allowed_channel_ids,
                    &policy,
                )
                .await?,
            )
        } else {
            None
        };
        let before = key_audit_for_user(transaction, id, user_id).await?;
        let updated_at = sqlx::query_scalar(
            "UPDATE api_keys \
             SET name=$3,status=$4,expires_at=$5, \
                 allowed_api_formats=CASE WHEN $6 THEN $7::api_format[] ELSE allowed_api_formats END, \
                 allowed_group_ids=$8,allowed_channel_ids=$9,requests_per_minute=$10, \
                 max_concurrent_requests=$11,quota_limit_amount=$12 \
             WHERE id=$1 AND user_id=$2 AND updated_at=$13 AND status <> 'revoked' \
             RETURNING updated_at",
        )
        .bind(id)
        .bind(user_id)
        .bind(&input.name)
        .bind(&input.status)
        .bind(input.expires_at)
        .bind(targets_changed)
        .bind(allowed_api_formats.unwrap_or_default())
        .bind(&input.allowed_group_ids)
        .bind(&input.allowed_channel_ids)
        .bind(input.requests_per_minute)
        .bind(input.max_concurrent_requests)
        .bind(input.quota_limit_amount)
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

    pub async fn update_users_batch(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        actor: Uuid,
        input: UserBatchUpdateInput,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        const MAX_BATCH_SIZE: usize = 100;

        if input.items.is_empty()
            || input.items.len() > MAX_BATCH_SIZE
            || input.changes.is_empty()
            || input
                .changes
                .status
                .as_deref()
                .is_some_and(|status| !matches!(status, "active" | "suspended" | "disabled"))
            || input.changes.balance.as_ref().is_some_and(|balance| {
                balance.amount.is_sign_negative()
                    || !matches!(balance.operation.as_str(), "set" | "increase" | "decrease")
            })
        {
            return Err(RepositoryError::Validation);
        }
        let mut ids = HashSet::with_capacity(input.items.len());
        if input.items.iter().any(|item| !ids.insert(item.id)) {
            return Err(RepositoryError::Validation);
        }
        if let Some(group_id) = input.changes.user_group_id {
            ensure_user_group_exists(transaction, group_id).await?;
        }
        if let Some(Some(policy_id)) = input.changes.default_api_key_policy_id {
            ensure_enabled_policy(transaction, policy_id).await?;
        }

        let balance_operation = input
            .changes
            .balance
            .as_ref()
            .map(|balance| balance.operation.as_str());
        let balance_amount = input.changes.balance.as_ref().map(|balance| balance.amount);
        let policy_present = input.changes.default_api_key_policy_id.is_some();
        let policy_id = input.changes.default_api_key_policy_id.flatten();
        let mut results = Vec::with_capacity(input.items.len());

        for item in input.items {
            let before = user_audit(transaction, item.id).await?;
            if !before["deleted_at"].is_null() {
                return Err(RepositoryError::NotFound);
            }
            let current_updated_at: DateTime<Utc> =
                serde_json::from_value(before["updated_at"].clone())
                    .map_err(|_| RepositoryError::Validation)?;
            if current_updated_at != item.updated_at {
                return Err(RepositoryError::Conflict);
            }
            let status_changed = input
                .changes
                .status
                .as_deref()
                .is_some_and(|status| before["status"].as_str() != Some(status));
            if let Some(next_status) = input.changes.status.as_deref() {
                validate_user_status_transition(
                    before["status"]
                        .as_str()
                        .ok_or(RepositoryError::Validation)?,
                    next_status,
                    before["can_reissue_invitation"].as_bool() == Some(true),
                )?;
                if item.id == actor && next_status != "active" {
                    return Err(RepositoryError::CannotDisableSelf);
                }
            }

            let updated_at = sqlx::query_scalar(
                "UPDATE users SET \
                 status=COALESCE($2,status), \
                 balance_amount=CASE $3::text \
                   WHEN 'set' THEN $4 \
                   WHEN 'increase' THEN balance_amount+$4 \
                   WHEN 'decrease' THEN balance_amount-$4 \
                   ELSE balance_amount \
                 END, \
                 user_group_id=COALESCE($5,user_group_id), \
                 default_api_key_policy_id=CASE \
                   WHEN $6 THEN $7::uuid ELSE default_api_key_policy_id \
                 END, \
                 auth_version=auth_version+CASE WHEN $8 THEN 1 ELSE 0 END \
                 WHERE id=$1 AND updated_at=$9 AND deleted_at IS NULL AND NOT is_system \
                 RETURNING updated_at",
            )
            .bind(item.id)
            .bind(&input.changes.status)
            .bind(balance_operation)
            .bind(balance_amount)
            .bind(input.changes.user_group_id)
            .bind(policy_present)
            .bind(policy_id)
            .bind(status_changed)
            .bind(item.updated_at)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?;

            if status_changed {
                sqlx::query(
                    "UPDATE user_sessions SET revoked_at=now() \
                     WHERE user_id=$1 AND revoked_at IS NULL",
                )
                .bind(item.id)
                .execute(&mut **transaction)
                .await?;
            }
            results.push(MutationResult {
                id: item.id,
                object_type: "user",
                action: "batch_update",
                before_redacted: before,
                after_redacted: user_audit(transaction, item.id).await?,
                created_secret: None,
                reason: None,
                updated_at,
                correlation_id: None,
            });
        }
        Ok(results)
    }

    pub async fn update_channels_batch(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        input: ChannelBatchUpdateInput,
    ) -> Result<Vec<MutationResult>, RepositoryError> {
        const MAX_BATCH_SIZE: usize = 100;

        if input.items.is_empty()
            || input.items.len() > MAX_BATCH_SIZE
            || input.changes.is_empty()
            || input.changes.weight.is_some_and(|weight| weight <= 0)
            || input
                .changes
                .billing_multiplier
                .is_some_and(|multiplier| multiplier.is_sign_negative())
        {
            return Err(RepositoryError::Validation);
        }
        let mut ids = HashSet::with_capacity(input.items.len());
        if input.items.iter().any(|item| !ids.insert(item.id)) {
            return Err(RepositoryError::Validation);
        }

        let mut results = Vec::with_capacity(input.items.len());
        for item in input.items {
            if channel_is_provider_managed(transaction, item.id).await? {
                return Err(RepositoryError::Validation);
            }
            let before = channel_audit(transaction, item.id).await?;
            let current_updated_at: DateTime<Utc> =
                serde_json::from_value(before["updated_at"].clone())
                    .map_err(|_| RepositoryError::Validation)?;
            if current_updated_at != item.updated_at {
                return Err(RepositoryError::Conflict);
            }
            let updated_at = sqlx::query_scalar(
                "UPDATE channels SET \
                 enabled=COALESCE($2,enabled), \
                 status_statistics_enabled=COALESCE($3,status_statistics_enabled), \
                 auto_disable_allowed=COALESCE($4,auto_disable_allowed), \
                 weight=COALESCE($5,weight), \
                 billing_multiplier=COALESCE($6,billing_multiplier) \
                 WHERE id=$1 AND updated_at=$7 RETURNING updated_at",
            )
            .bind(item.id)
            .bind(input.changes.enabled)
            .bind(input.changes.status_statistics_enabled)
            .bind(input.changes.auto_disable_allowed)
            .bind(input.changes.weight)
            .bind(input.changes.billing_multiplier)
            .bind(item.updated_at)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Conflict)?;
            results.push(MutationResult {
                id: item.id,
                object_type: "channel",
                action: "batch_update",
                before_redacted: before,
                after_redacted: channel_audit(transaction, item.id).await?,
                created_secret: None,
                reason: None,
                updated_at,
                correlation_id: None,
            });
        }
        Ok(results)
    }

    pub async fn apply_control_plane_mutation(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        mutation: ControlPlaneMutation,
    ) -> Result<MutationResult, RepositoryError> {
        match mutation {
            ControlPlaneMutation::CreateUser(input) => {
                user_create(transaction, Uuid::new_v4(), input).await
            }
            ControlPlaneMutation::UpdateUser {
                id,
                input,
                expected_updated_at,
            } => user_update(transaction, id, input, expected_updated_at).await,
            ControlPlaneMutation::DeleteUser {
                id,
                deleted_by,
                expected_updated_at,
            } => user_soft_delete(transaction, id, deleted_by, expected_updated_at).await,
            ControlPlaneMutation::CreateUserGroup(input) => {
                user_group_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateUserGroup {
                id,
                input,
                expected_updated_at,
            } => user_group_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::DeleteUserGroup {
                id,
                expected_updated_at,
            } => user_group_delete(transaction, id, expected_updated_at).await,
            ControlPlaneMutation::CreateModel(input) => {
                model_insert(transaction, Uuid::new_v4(), input, true, None).await
            }
            ControlPlaneMutation::UpdateModel {
                id,
                input,
                expected_updated_at,
            } => model_insert(transaction, id, input, false, Some(expected_updated_at)).await,
            ControlPlaneMutation::CreateApiKey(input) => {
                validate_admin_api_key_input(
                    &input.name,
                    &input.allowed_api_formats,
                    &input.permissions,
                    &input.allowed_group_ids,
                    &input.allowed_channel_ids,
                    input.requests_per_minute,
                    input.max_concurrent_requests,
                    input.quota_limit_amount,
                )?;
                let id = Uuid::new_v4();
                let secret = generate_api_key_secret();
                let updated_at = sqlx::query_scalar("INSERT INTO api_keys (id, user_id, name, secret_value, status, expires_at, allowed_api_formats, permissions, allowed_group_ids, allowed_channel_ids, requests_per_minute, max_concurrent_requests, quota_limit_amount) VALUES ($1,$2,$3,$4,'active',$5,$6::api_format[],$7,$8,$9,$10,$11,$12) RETURNING updated_at")
                    .bind(id).bind(input.user_id).bind(&input.name).bind(&secret).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).bind(&input.allowed_channel_ids).bind(input.requests_per_minute).bind(input.max_concurrent_requests).bind(input.quota_limit_amount).fetch_one(&mut **transaction).await?;
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
                validate_admin_api_key_input(
                    &input.name,
                    &input.allowed_api_formats,
                    &input.permissions,
                    &input.allowed_group_ids,
                    &input.allowed_channel_ids,
                    input.requests_per_minute,
                    input.max_concurrent_requests,
                    input.quota_limit_amount,
                )?;
                let before = key_audit(transaction, id).await?;
                let updated_at = sqlx::query_scalar("UPDATE api_keys SET name=$2,status=$3,expires_at=$4,allowed_api_formats=$5::api_format[],permissions=$6,allowed_group_ids=$7,allowed_channel_ids=$8,requests_per_minute=$9,max_concurrent_requests=$10,quota_limit_amount=$11 WHERE id=$1 AND updated_at=$12 AND NOT (status='revoked' AND $3 <> 'revoked') RETURNING updated_at")
                    .bind(id).bind(&input.name).bind(&input.status).bind(input.expires_at).bind(&input.allowed_api_formats).bind(&input.permissions).bind(&input.allowed_group_ids).bind(&input.allowed_channel_ids).bind(input.requests_per_minute).bind(input.max_concurrent_requests).bind(input.quota_limit_amount).bind(expected_updated_at).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?;
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
            ControlPlaneMutation::RecoverChannel {
                id,
                expected_updated_at,
            } => channel_recover(transaction, id, expected_updated_at).await,
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
            ControlPlaneMutation::DeleteProxy {
                id,
                expected_updated_at,
            } => proxy_delete(transaction, id, expected_updated_at).await,
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
            ControlPlaneMutation::UpdateSystemSettings {
                input,
                expected_updated_at,
            } => system_settings_update(transaction, input, expected_updated_at).await,
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
    pub async fn insert_system_audit(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        mutation: &MutationResult,
        correlation_id: Uuid,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "INSERT INTO audit_logs
             (id,actor_type,action,object_type,object_id,before_redacted,after_redacted,correlation_id,reason)
             VALUES ($1,'system',$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(Uuid::new_v4())
        .bind(mutation.action)
        .bind(mutation.object_type)
        .bind(mutation.id)
        .bind(&mutation.before_redacted)
        .bind(&mutation.after_redacted)
        .bind(correlation_id.to_string())
        .bind(&mutation.reason)
        .execute(&mut **transaction)
        .await?;
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
    id: Uuid,
    name: String,
    allowed_group_ids: Vec<Uuid>,
    allowed_channel_ids: Vec<Uuid>,
    enabled: bool,
}

#[derive(FromRow)]
struct SelfApiKeyCurrent {
    allowed_group_ids: Vec<Uuid>,
    allowed_channel_ids: Vec<Uuid>,
}

#[derive(FromRow)]
struct ApiKeyTargetGroup {
    id: Uuid,
    api_format: String,
}

#[derive(FromRow)]
struct ApiKeyTargetChannel {
    id: Uuid,
    channel_group_id: Uuid,
    api_format: String,
}

async fn load_self_api_key_policy(
    transaction: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
) -> Result<SelfApiKeyPolicy, RepositoryError> {
    let policy = sqlx::query_as::<_, SelfApiKeyPolicy>(
        "SELECT p.id,p.name,p.allowed_group_ids,p.allowed_channel_ids,p.enabled \
         FROM users AS u \
         JOIN user_groups AS g ON g.id=u.user_group_id \
         JOIN api_key_policies AS p \
           ON p.id=COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id) \
         WHERE u.id=$1 AND u.status='active' AND u.deleted_at IS NULL \
         FOR UPDATE OF u,g,p",
    )
    .bind(user_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::DefaultApiKeyPolicyRequired)?;
    ensure_policy_enabled(&policy)?;
    Ok(policy)
}

fn ensure_policy_enabled(policy: &SelfApiKeyPolicy) -> Result<(), RepositoryError> {
    if policy.enabled {
        Ok(())
    } else {
        Err(RepositoryError::DefaultApiKeyPolicyDisabled)
    }
}

fn same_uuid_set(left: &[Uuid], right: &[Uuid]) -> bool {
    left.len() == right.len()
        && left.iter().copied().collect::<HashSet<_>>()
            == right.iter().copied().collect::<HashSet<_>>()
}

fn validate_target_lists(
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    allow_empty: bool,
) -> Result<(), RepositoryError> {
    if (!allow_empty && allowed_group_ids.is_empty() && allowed_channel_ids.is_empty())
        || allowed_group_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != allowed_group_ids.len()
        || allowed_channel_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .len()
            != allowed_channel_ids.len()
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn validate_api_key_limits(
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
) -> Result<(), RepositoryError> {
    if requests_per_minute.is_some_and(|value| value <= 0)
        || max_concurrent_requests.is_some_and(|value| value <= 0)
        || quota_limit_amount.is_some_and(|value| value.is_sign_negative())
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn validate_self_api_key_input(
    name: &str,
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    allow_empty_targets: bool,
) -> Result<(), RepositoryError> {
    if name.trim().is_empty() {
        return Err(RepositoryError::Validation);
    }
    validate_target_lists(allowed_group_ids, allowed_channel_ids, allow_empty_targets)?;
    validate_api_key_limits(
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_admin_api_key_input(
    name: &str,
    allowed_api_formats: &[String],
    permissions: &[String],
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
) -> Result<(), RepositoryError> {
    if name.trim().is_empty()
        || allowed_api_formats.is_empty()
        || permissions.is_empty()
        || allowed_api_formats.iter().collect::<HashSet<_>>().len() != allowed_api_formats.len()
        || permissions.iter().collect::<HashSet<_>>().len() != permissions.len()
    {
        return Err(RepositoryError::Validation);
    }
    validate_target_lists(allowed_group_ids, allowed_channel_ids, false)?;
    validate_api_key_limits(
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
    )
}

async fn resolve_self_api_key_targets(
    transaction: &mut Transaction<'_, Postgres>,
    selected_group_ids: &[Uuid],
    selected_channel_ids: &[Uuid],
    policy: &SelfApiKeyPolicy,
) -> Result<Vec<String>, RepositoryError> {
    let groups = sqlx::query_as::<_, ApiKeyTargetGroup>(
        "SELECT id,api_format::text AS api_format \
         FROM channel_groups \
         WHERE id = ANY($1)",
    )
    .bind(selected_group_ids)
    .fetch_all(&mut **transaction)
    .await?;
    let channels = sqlx::query_as::<_, ApiKeyTargetChannel>(
        "SELECT id,channel_group_id,api_format::text AS api_format \
         FROM channels \
         WHERE id = ANY($1)",
    )
    .bind(selected_channel_ids)
    .fetch_all(&mut **transaction)
    .await?;
    if groups.len() != selected_group_ids.len() || channels.len() != selected_channel_ids.len() {
        return Err(RepositoryError::ApiKeyTargetNotAllowed);
    }

    let allowed_groups = policy
        .allowed_group_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    let allowed_channels = policy
        .allowed_channel_ids
        .iter()
        .copied()
        .collect::<HashSet<_>>();
    if groups
        .iter()
        .any(|group| !allowed_groups.contains(&group.id))
        || channels.iter().any(|channel| {
            !allowed_groups.contains(&channel.channel_group_id)
                && !allowed_channels.contains(&channel.id)
        })
    {
        return Err(RepositoryError::ApiKeyTargetNotAllowed);
    }

    let mut formats = BTreeSet::new();
    formats.extend(groups.into_iter().map(|group| group.api_format));
    formats.extend(channels.into_iter().map(|channel| channel.api_format));
    if formats.is_empty() {
        return Err(RepositoryError::Validation);
    }
    Ok(formats.into_iter().collect())
}

async fn validate_policy_targets(
    transaction: &mut Transaction<'_, Postgres>,
    allowed_group_ids: &[Uuid],
    allowed_channel_ids: &[Uuid],
) -> Result<(), RepositoryError> {
    validate_target_lists(allowed_group_ids, allowed_channel_ids, false)?;
    let group_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM channel_groups WHERE id = ANY($1)")
            .bind(allowed_group_ids)
            .fetch_one(&mut **transaction)
            .await?;
    let channel_count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM channels WHERE id = ANY($1)")
            .bind(allowed_channel_ids)
            .fetch_one(&mut **transaction)
            .await?;
    if group_count != allowed_group_ids.len() as i64
        || channel_count != allowed_channel_ids.len() as i64
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

async fn key_audit_for_user(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    user_id: Uuid,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'user_id',user_id,'name',name,'status',status,'expires_at',expires_at,'allowed_api_formats',allowed_api_formats,'permissions',permissions,'allowed_group_ids',allowed_group_ids,'allowed_channel_ids',allowed_channel_ids,'requests_per_minute',requests_per_minute,'max_concurrent_requests',max_concurrent_requests,'quota_limit_amount',quota_limit_amount,'quota_used_amount',quota_used_amount,'created_at',created_at,'updated_at',updated_at) FROM api_keys WHERE id=$1 AND user_id=$2 AND NOT is_system FOR UPDATE",
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
        "SELECT json_build_object('id',id,'user_id',user_id,'name',name,'status',status,'expires_at',expires_at,'allowed_api_formats',allowed_api_formats,'permissions',permissions,'allowed_group_ids',allowed_group_ids,'allowed_channel_ids',allowed_channel_ids,'requests_per_minute',requests_per_minute,'max_concurrent_requests',max_concurrent_requests,'quota_limit_amount',quota_limit_amount,'quota_used_amount',quota_used_amount,'created_at',created_at,'updated_at',updated_at) FROM api_keys WHERE id=$1 AND NOT is_system FOR UPDATE",
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
        "SELECT json_build_object( \
            'id',u.id, \
            'email',u.email, \
            'display_name',u.display_name, \
            'role',u.role, \
            'status',u.status, \
            'can_reissue_invitation',(u.password_hash IS NULL AND u.email IS NOT NULL AND u.status IN ('invited','suspended','disabled')), \
            'user_group_id',u.user_group_id, \
            'user_group_system_role',g.system_role, \
            'default_api_key_policy_id',u.default_api_key_policy_id, \
            'effective_api_key_policy_id',COALESCE(u.default_api_key_policy_id,g.default_api_key_policy_id), \
            'balance_amount',u.balance_amount, \
            'deleted_at',u.deleted_at, \
            'deleted_by',u.deleted_by, \
            'created_at',u.created_at, \
            'updated_at',u.updated_at \
         ) FROM users AS u \
         JOIN user_groups AS g ON g.id=u.user_group_id \
         WHERE u.id=$1 AND NOT u.is_system FOR UPDATE OF u,g",
    )
    .bind(id)
    .fetch_optional(&mut **transaction)
    .await?;
    value.ok_or(RepositoryError::NotFound)
}
async fn user_group_audit(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<Value, RepositoryError> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT true FROM user_groups WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **transaction)
            .await?;
    if exists.is_none() {
        return Err(RepositoryError::NotFound);
    }
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object( \
            'id',g.id, \
            'name',g.name, \
            'description',g.description, \
            'default_api_key_policy_id',g.default_api_key_policy_id, \
            'system_role',g.system_role, \
            'member_count',count(u.id) FILTER (WHERE u.deleted_at IS NULL AND NOT u.is_system), \
            'created_at',g.created_at, \
            'updated_at',g.updated_at \
         ) \
         FROM user_groups AS g \
         LEFT JOIN users AS u ON u.user_group_id=g.id \
         WHERE g.id=$1 \
         GROUP BY g.id",
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
        "SELECT json_build_object('id',id,'source_model_id',source_model_id,'display_name',display_name,'provider_name',provider_name,'enabled',enabled,'price_unit_tokens',price_unit_tokens,'input_unit_price',input_unit_price,'cached_input_unit_price',cached_input_unit_price,'cache_write_unit_price',cache_write_unit_price,'output_unit_price',output_unit_price,'price_effective_at',price_effective_at,'advanced_billing',advanced_billing,'last_synced_at',last_synced_at,'created_at',created_at,'updated_at',updated_at) FROM models WHERE id=$1 FOR UPDATE",
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
    // Audit snapshots remain allowlisted even though authorized detail reads
    // expose the stored credential and transform document for editing.
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT json_build_object('id',id,'channel_group_id',channel_group_id,'api_format',api_format,'name',name,'base_url',base_url,'enabled',enabled,'supports_websocket',supports_websocket,'status_statistics_enabled',status_statistics_enabled,'auto_disabled',auto_disabled,'auto_disabled_reason',auto_disabled_reason,'auto_disable_allowed',auto_disable_allowed,'weight',weight,'billing_multiplier',billing_multiplier,'proxy_id',proxy_id,'config_template_id',config_template_id,'connect_timeout_ms',connect_timeout_ms,'response_header_timeout_ms',response_header_timeout_ms,'stream_idle_timeout_ms',stream_idle_timeout_ms,'upstream_auth_kind',upstream_auth_kind,'upstream_auth_header_name',upstream_auth_header_name,'upstream_credential_configured',(upstream_api_key IS NOT NULL),'available_models',available_models,'test_model',test_model,'created_at',created_at,'updated_at',updated_at) FROM channels WHERE id=$1 FOR UPDATE",
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
        "SELECT json_build_object('id',id,'name',name,'allowed_group_ids',allowed_group_ids,'allowed_channel_ids',allowed_channel_ids,'enabled',enabled,'created_at',created_at,'updated_at',updated_at) FROM api_key_policies WHERE id=$1 FOR UPDATE",
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
    if input.name.trim().is_empty() {
        return Err(RepositoryError::Validation);
    }
    validate_policy_targets(
        transaction,
        &input.allowed_group_ids,
        &input.allowed_channel_ids,
    )
    .await?;
    let before = if create {
        json!({})
    } else {
        api_key_policy_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar(
            "INSERT INTO api_key_policies \
             (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
             VALUES ($1,$2,$3,$4,$5) RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.allowed_group_ids)
        .bind(&input.allowed_channel_ids)
        .bind(input.enabled)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE api_key_policies \
             SET name=$2,allowed_group_ids=$3,allowed_channel_ids=$4,enabled=$5 \
             WHERE id=$1 AND updated_at=$6 RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.allowed_group_ids)
        .bind(&input.allowed_channel_ids)
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
    if input.name.trim().is_empty()
        || input.priority < 0
        || !matches!(
            input.selection_strategy.as_str(),
            "weighted_random" | "weighted_round_robin"
        )
        || !matches!(
            input.connector_kind.as_str(),
            "openai_compatible" | "codex_oauth"
        )
        || (input.connector_kind == "codex_oauth" && input.api_format != "open_ai_responses")
    {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        group_audit(transaction, id).await?
    };
    if !create && before["connector_kind"].as_str() != Some(input.connector_kind.as_str()) {
        return Err(RepositoryError::Validation);
    }
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO channel_groups (id,name,api_format,connector_kind,priority,selection_strategy,enabled) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7) RETURNING updated_at").bind(id).bind(&input.name).bind(&input.api_format).bind(&input.connector_kind).bind(input.priority).bind(&input.selection_strategy).bind(input.enabled).fetch_one(&mut **transaction).await?
    } else {
        sqlx::query_scalar("UPDATE channel_groups SET name=$2,api_format=$3::api_format,connector_kind=$4,priority=$5,selection_strategy=$6,enabled=$7 WHERE id=$1 AND updated_at=$8 RETURNING updated_at").bind(id).bind(&input.name).bind(&input.api_format).bind(&input.connector_kind).bind(input.priority).bind(&input.selection_strategy).bind(input.enabled).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
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

fn default_user_group_id(role: &str) -> Result<Uuid, RepositoryError> {
    match role {
        "user" => Ok(DEFAULT_USER_GROUP_ID),
        "admin" => Ok(DEFAULT_ADMIN_GROUP_ID),
        _ => Err(RepositoryError::Validation),
    }
}

async fn ensure_user_group_exists(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), RepositoryError> {
    let exists =
        sqlx::query_scalar::<_, bool>("SELECT EXISTS(SELECT 1 FROM user_groups WHERE id=$1)")
            .bind(id)
            .fetch_one(&mut **transaction)
            .await?;
    if exists {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn resolve_user_group_id(
    transaction: &mut Transaction<'_, Postgres>,
    requested: Option<Uuid>,
    role: &str,
) -> Result<Uuid, RepositoryError> {
    let id = requested.unwrap_or(default_user_group_id(role)?);
    ensure_user_group_exists(transaction, id).await?;
    Ok(id)
}

async fn ensure_enabled_policy(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
) -> Result<(), RepositoryError> {
    let enabled = sqlx::query_scalar::<_, bool>("SELECT enabled FROM api_key_policies WHERE id=$1")
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Validation)?;
    if enabled {
        Ok(())
    } else {
        Err(RepositoryError::Validation)
    }
}

async fn user_group_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: UserGroupInput,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    if input.name.trim().is_empty()
        || input.name.len() > 100
        || input
            .description
            .as_ref()
            .is_some_and(|description| description.len() > 500)
    {
        return Err(RepositoryError::Validation);
    }
    let before = if create {
        json!({})
    } else {
        user_group_audit(transaction, id).await?
    };
    if let Some(policy_id) = input.default_api_key_policy_id {
        let current_policy_id = before["default_api_key_policy_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok());
        if create || current_policy_id != Some(policy_id) {
            ensure_enabled_policy(transaction, policy_id).await?;
        }
    }
    let updated_at = if create {
        sqlx::query_scalar(
            "INSERT INTO user_groups \
             (id,name,description,default_api_key_policy_id) \
             VALUES ($1,$2,$3,$4) RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(input.default_api_key_policy_id)
        .fetch_one(&mut **transaction)
        .await?
    } else {
        sqlx::query_scalar(
            "UPDATE user_groups SET \
             name=$2,description=$3,default_api_key_policy_id=$4 \
             WHERE id=$1 AND updated_at=$5 RETURNING updated_at",
        )
        .bind(id)
        .bind(&input.name)
        .bind(&input.description)
        .bind(input.default_api_key_policy_id)
        .bind(expected_updated_at.expect("PUT version"))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    };
    Ok(MutationResult {
        id,
        object_type: "user_group",
        action: if create { "create" } else { "update" },
        before_redacted: before,
        after_redacted: user_group_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn user_group_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = user_group_audit(transaction, id).await?;
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    if !before["system_role"].is_null() {
        return Err(RepositoryError::ProtectedUserGroup);
    }
    if before["member_count"].as_i64().unwrap_or_default() > 0 {
        return Err(RepositoryError::UserGroupInUse);
    }
    let registration_code_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM registration_invitation_codes WHERE user_group_id=$1",
    )
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?;
    if registration_code_count > 0 {
        return Err(RepositoryError::UserGroupInUse);
    }
    let deleted = sqlx::query("DELETE FROM user_groups WHERE id=$1 AND updated_at=$2")
        .bind(id)
        .bind(expected_updated_at)
        .execute(&mut **transaction)
        .await?;
    if deleted.rows_affected() != 1 {
        return Err(RepositoryError::Conflict);
    }
    Ok(MutationResult {
        id,
        object_type: "user_group",
        action: "delete",
        before_redacted: before,
        after_redacted: json!({}),
        created_secret: None,
        reason: None,
        updated_at: expected_updated_at,
        correlation_id: None,
    })
}

async fn user_create(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: UserInput,
) -> Result<MutationResult, RepositoryError> {
    let user_group_id =
        resolve_user_group_id(transaction, input.user_group_id, &input.role).await?;
    if let Some(policy_id) = input.default_api_key_policy_id {
        ensure_enabled_policy(transaction, policy_id).await?;
    }
    let updated_at = sqlx::query_scalar(
        "INSERT INTO users \
         (id,email,display_name,role,status,balance_amount,user_group_id,default_api_key_policy_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING updated_at",
    )
    .bind(id)
    .bind(&input.email)
    .bind(&input.display_name)
    .bind(&input.role)
    .bind(&input.status)
    .bind(input.balance_amount)
    .bind(user_group_id)
    .bind(input.default_api_key_policy_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(MutationResult {
        id,
        object_type: "user",
        action: "create",
        before_redacted: json!({}),
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn user_update(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: UserUpdateInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.is_empty() {
        return Err(RepositoryError::Validation);
    }

    let before = user_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    validate_user_update(transaction, &before, &input).await?;

    let email_changed = input
        .email
        .as_ref()
        .is_some_and(|email| before["email"].as_str() != email.as_deref());
    let role_changed = input
        .role
        .as_deref()
        .is_some_and(|role| before["role"].as_str() != Some(role));
    let status_changed = input
        .status
        .as_deref()
        .is_some_and(|status| before["status"].as_str() != Some(status));
    let invalidates_sessions = email_changed || role_changed || status_changed;
    let resolved_group_id = if let Some(group_id) = input.user_group_id {
        Some(group_id)
    } else if role_changed && before["user_group_system_role"].as_str() == before["role"].as_str() {
        Some(default_user_group_id(
            input.role.as_deref().ok_or(RepositoryError::Validation)?,
        )?)
    } else {
        None
    };
    if let Some(group_id) = resolved_group_id {
        ensure_user_group_exists(transaction, group_id).await?;
    }

    let UserUpdateInput {
        display_name,
        email,
        role,
        status,
        balance_amount,
        user_group_id: _,
        default_api_key_policy_id,
    } = input;
    let email_present = email.is_some();
    let email = email.flatten();
    let policy_present = default_api_key_policy_id.is_some();
    let default_api_key_policy_id = default_api_key_policy_id.flatten();

    let updated_at = sqlx::query_scalar(
        "UPDATE users SET \
         email=CASE WHEN $2 THEN $3::varchar ELSE email END, \
         display_name=COALESCE($4,display_name), \
         role=COALESCE($5,role), \
         status=COALESCE($6,status), \
         balance_amount=COALESCE($7,balance_amount), \
         user_group_id=COALESCE($8,user_group_id), \
         default_api_key_policy_id=CASE WHEN $9 THEN $10::uuid ELSE default_api_key_policy_id END, \
         auth_version=auth_version+CASE WHEN $11 THEN 1 ELSE 0 END \
         WHERE id=$1 AND updated_at=$12 AND deleted_at IS NULL RETURNING updated_at",
    )
    .bind(id)
    .bind(email_present)
    .bind(email)
    .bind(display_name)
    .bind(role)
    .bind(status)
    .bind(balance_amount)
    .bind(resolved_group_id)
    .bind(policy_present)
    .bind(default_api_key_policy_id)
    .bind(invalidates_sessions)
    .bind(expected_updated_at)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;

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
        action: "update",
        before_redacted: before,
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn validate_user_update(
    transaction: &mut Transaction<'_, Postgres>,
    before: &Value,
    input: &UserUpdateInput,
) -> Result<(), RepositoryError> {
    if input
        .display_name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 200)
        || input.email.as_ref().is_some_and(|email| {
            email.as_ref().is_some_and(|email| {
                let email = email.trim();
                email.is_empty()
                    || email.len() > 320
                    || email.bytes().any(|byte| byte.is_ascii_whitespace())
                    || !email.contains('@')
            })
        })
        || input
            .role
            .as_deref()
            .is_some_and(|role| !matches!(role, "user" | "admin"))
    {
        return Err(RepositoryError::Validation);
    }

    if let Some(next_status) = input.status.as_deref() {
        let current_status = before["status"]
            .as_str()
            .ok_or(RepositoryError::Validation)?;
        validate_user_status_transition(
            current_status,
            next_status,
            before["can_reissue_invitation"].as_bool() == Some(true),
        )?;
    }

    if let Some(Some(policy_id)) = input.default_api_key_policy_id {
        let current_policy_id = before["default_api_key_policy_id"]
            .as_str()
            .and_then(|value| Uuid::parse_str(value).ok());
        if current_policy_id != Some(policy_id) {
            ensure_enabled_policy(transaction, policy_id).await?;
        }
    }
    if let Some(group_id) = input.user_group_id {
        ensure_user_group_exists(transaction, group_id).await?;
    }

    Ok(())
}

fn validate_user_status_transition(
    current_status: &str,
    next_status: &str,
    can_reissue_invitation: bool,
) -> Result<(), RepositoryError> {
    if !matches!(next_status, "active" | "suspended" | "disabled") {
        return Err(RepositoryError::Validation);
    }
    if current_status != next_status
        && !matches!(current_status, "active" | "suspended" | "disabled")
    {
        return Err(RepositoryError::Validation);
    }
    if next_status == "active" && current_status != "active" && can_reissue_invitation {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

async fn user_soft_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    deleted_by: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if id == deleted_by {
        return Err(RepositoryError::CannotDeleteSelf);
    }
    let before = user_audit(transaction, id).await?;
    if !before["deleted_at"].is_null() {
        return Err(RepositoryError::NotFound);
    }
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    if before["role"].as_str() == Some("admin") && before["status"].as_str() == Some("active") {
        let remaining = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM users \
             WHERE role='admin' AND status='active' AND NOT is_system \
               AND deleted_at IS NULL AND id<>$1",
        )
        .bind(id)
        .fetch_one(&mut **transaction)
        .await?;
        if remaining == 0 {
            return Err(RepositoryError::LastAdministrator);
        }
    }

    let deleted_name = format!("Deleted user {}", id.simple());
    let updated_at = sqlx::query_scalar(
        "UPDATE users SET \
         email=NULL,display_name=$2,role='user',status='disabled',password_hash=NULL, \
         auth_version=auth_version+1,user_group_id=$3,default_api_key_policy_id=NULL, \
         deleted_at=now(),deleted_by=$4 \
         WHERE id=$1 AND updated_at=$5 AND deleted_at IS NULL AND NOT is_system \
         RETURNING updated_at",
    )
    .bind(id)
    .bind(deleted_name)
    .bind(DEFAULT_USER_GROUP_ID)
    .bind(deleted_by)
    .bind(expected_updated_at)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    sqlx::query(
        "UPDATE user_sessions SET revoked_at=now() \
         WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE user_invitations SET revoked_at=now() \
         WHERE user_id=$1 AND accepted_at IS NULL AND revoked_at IS NULL",
    )
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE api_keys SET status='revoked' \
         WHERE user_id=$1 AND status<>'revoked' AND NOT is_system",
    )
    .bind(id)
    .execute(&mut **transaction)
    .await?;

    Ok(MutationResult {
        id,
        object_type: "user",
        action: "delete",
        before_redacted: before,
        after_redacted: user_audit(transaction, id).await?,
        created_secret: None,
        reason: Some("user anonymized and credentials revoked".into()),
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
    if input.advanced_billing.as_ref().is_some_and(|billing| {
        crate::domain::CompiledAdvancedBilling::compile(billing.clone()).is_err()
    }) {
        return Err(RepositoryError::Validation);
    }
    let advanced_billing_present = input.advanced_billing.is_some();
    let advanced_billing = serde_json::to_value(input.advanced_billing.unwrap_or_default())
        .expect("advanced billing serializes");
    let source_payload_present = input.source_payload.is_some();
    let source_payload = input.source_payload.unwrap_or_else(empty_object);
    let before = if create {
        json!({})
    } else {
        model_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO models (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,advanced_billing,source_payload) VALUES ($1,$2,$3,$4,$5,'USD',$6,$7,$8,$9,$10,$11,$12,$13) RETURNING updated_at")
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
            .bind(&advanced_billing)
            .bind(&source_payload)
            .fetch_one(&mut **transaction)
            .await?
    } else {
        sqlx::query_scalar("UPDATE models SET source_model_id=$2,display_name=$3,provider_name=$4,enabled=$5,currency='USD',price_unit_tokens=$6,input_unit_price=$7,cached_input_unit_price=$8,cache_write_unit_price=$9,output_unit_price=$10,price_effective_at=$11,advanced_billing=CASE WHEN $12 THEN $13 ELSE advanced_billing END,source_payload=CASE WHEN $14 THEN $15 ELSE source_payload END WHERE id=$1 AND updated_at=$16 RETURNING updated_at")
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
            .bind(advanced_billing_present)
            .bind(&advanced_billing)
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
    let advanced_billing =
        serde_json::to_value(&input.advanced_billing).expect("advanced billing serializes");
    let updated_at = sqlx::query_scalar("INSERT INTO models (id,source_model_id,display_name,provider_name,enabled,currency,price_unit_tokens,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at,advanced_billing,source_payload,last_synced_at) VALUES ($1,$2,$3,$4,true,'USD',1000000,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING updated_at")
        .bind(id)
        .bind(&input.source_model_id)
        .bind(&input.display_name)
        .bind(&input.provider_name)
        .bind(input.input_unit_price)
        .bind(input.cached_input_unit_price)
        .bind(input.cache_write_unit_price)
        .bind(input.output_unit_price)
        .bind(synced_at)
        .bind(&advanced_billing)
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
/// Refreshes catalog-owned price facts, long-context tiers, and any available
/// request multipliers for a local source model. Display name, provider label,
/// enabled state, and unmatched local request-multiplier rules remain
/// administrator-managed.
async fn sync_model_price(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: SyncedModelInput,
    synced_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    if input.source_payload.as_object().is_none() {
        return Err(RepositoryError::Validation);
    }
    let current_advanced_billing: Value = sqlx::query_scalar(
        "SELECT advanced_billing
         FROM models
         WHERE id=$1 AND source_model_id=$2
         FOR UPDATE",
    )
    .bind(id)
    .bind(&input.source_model_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    let before = model_audit(transaction, id).await?;
    let advanced_billing =
        merge_synced_advanced_billing(current_advanced_billing, input.advanced_billing)?;
    let updated_at = sqlx::query_scalar(
        "UPDATE models
         SET currency='USD',
             price_unit_tokens=1000000,
             input_unit_price=$3,
             cached_input_unit_price=$4,
             cache_write_unit_price=$5,
             output_unit_price=$6,
             price_effective_at=$7,
             advanced_billing=$8,
             source_payload=$9,
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
    .bind(&advanced_billing)
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

fn merge_synced_advanced_billing(
    current: Value,
    synced: crate::domain::AdvancedBilling,
) -> Result<Value, RepositoryError> {
    let mut merged = serde_json::from_value::<crate::domain::AdvancedBilling>(current)
        .map_err(|_| RepositoryError::Validation)?;
    merged.long_context_tiers = synced.long_context_tiers;
    if !synced.request_multipliers.is_empty() {
        merged.request_multipliers.retain(|current| {
            !synced.request_multipliers.iter().any(|catalog| {
                current.json_pointer == catalog.json_pointer && current.value == catalog.value
            })
        });
        merged
            .request_multipliers
            .extend(synced.request_multipliers);
    }
    crate::domain::CompiledAdvancedBilling::compile(merged.clone())
        .map_err(|_| RepositoryError::Validation)?;
    serde_json::to_value(merged).map_err(|_| RepositoryError::Validation)
}

#[cfg(test)]
mod synced_advanced_billing_tests {
    use rust_decimal::Decimal;
    use serde_json::json;

    use super::merge_synced_advanced_billing;
    use crate::domain::{AdvancedBilling, LongContextTier, RequestBillingMultiplier};

    #[test]
    fn sync_without_catalog_request_rules_preserves_local_multipliers() {
        let current = serde_json::to_value(AdvancedBilling {
            long_context_tiers: vec![],
            request_multipliers: vec![RequestBillingMultiplier {
                json_pointer: "/reasoning/effort".into(),
                value: json!("high"),
                multiplier: Decimal::from(2),
            }],
        })
        .unwrap();
        let merged = merge_synced_advanced_billing(
            current,
            AdvancedBilling {
                long_context_tiers: vec![LongContextTier {
                    input_tokens_threshold: 128_000,
                    input_unit_price: Decimal::from(3),
                    cached_input_unit_price: Decimal::new(3, 1),
                    cache_write_unit_price: Decimal::ZERO,
                    output_unit_price: Some(Decimal::from(6)),
                }],
                request_multipliers: vec![],
            },
        )
        .unwrap();
        let merged: AdvancedBilling = serde_json::from_value(merged).unwrap();

        assert_eq!(merged.long_context_tiers[0].input_tokens_threshold, 128_000);
        assert_eq!(merged.request_multipliers.len(), 1);
        assert_eq!(
            merged.request_multipliers[0].json_pointer,
            "/reasoning/effort"
        );
    }
}

async fn channel_insert(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: impl Into<ChannelMutationInput>,
    create: bool,
    expected_updated_at: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let input = input.into();
    if !create && channel_is_provider_managed(transaction, id).await? {
        return Err(RepositoryError::Validation);
    }
    let connector_kind =
        sqlx::query_scalar::<_, String>("SELECT connector_kind FROM channel_groups WHERE id=$1")
            .bind(input.channel_group_id)
            .fetch_optional(&mut **transaction)
            .await?
            .ok_or(RepositoryError::Validation)?;
    if connector_kind != "openai_compatible" {
        return Err(RepositoryError::Validation);
    }
    if input
        .override_document
        .as_ref()
        .is_some_and(|document| document.as_object().is_none())
    {
        return Err(RepositoryError::Validation);
    }
    if matches!(input.upstream_api_key, Some(None)) && input.upstream_auth_kind != "none" {
        return Err(RepositoryError::Validation);
    }
    if input
        .billing_multiplier
        .is_some_and(|multiplier| multiplier.is_sign_negative())
    {
        return Err(RepositoryError::Validation);
    }
    if input.supports_websocket && input.api_format != "open_ai_responses" {
        return Err(RepositoryError::Validation);
    }
    if input.test_model.as_ref().is_some_and(|model| {
        !input
            .available_models
            .iter()
            .any(|available| available == model)
    }) {
        return Err(RepositoryError::Validation);
    }
    if let Some(test_model) = input.test_model.as_deref() {
        let configured = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM models WHERE source_model_id=$1)",
        )
        .bind(test_model)
        .fetch_one(&mut **transaction)
        .await?;
        if !configured {
            return Err(RepositoryError::Validation);
        }
    }
    let override_document_present = input.override_document.is_some();
    let override_document = input.override_document.unwrap_or_else(empty_object);
    let before = if create {
        json!({})
    } else {
        channel_audit(transaction, id).await?
    };
    let updated_at = if create {
        sqlx::query_scalar("INSERT INTO channels (id,channel_group_id,api_format,name,base_url,enabled,weight,billing_multiplier,proxy_id,config_template_id,override_document,connect_timeout_ms,response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,upstream_auth_header_name,upstream_api_key,available_models,test_model,status_statistics_enabled,auto_disable_allowed,supports_websocket) VALUES ($1,$2,$3::api_format,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22) RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.billing_multiplier.unwrap_or_else(default_billing_multiplier)).bind(input.proxy_id).bind(input.config_template_id).bind(&override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.test_model).bind(input.status_statistics_enabled).bind(input.auto_disable_allowed).bind(input.supports_websocket).fetch_one(&mut **transaction).await?
    } else {
        let credential_present = input.upstream_api_key.is_some();
        sqlx::query_scalar("UPDATE channels SET channel_group_id=$2,api_format=$3::api_format,name=$4,base_url=$5,enabled=$6,weight=$7,billing_multiplier=COALESCE($8,billing_multiplier),proxy_id=$9,config_template_id=$10,override_document=CASE WHEN $11 THEN $12 ELSE override_document END,connect_timeout_ms=$13,response_header_timeout_ms=$14,stream_idle_timeout_ms=$15,upstream_auth_kind=$16,upstream_auth_header_name=$17,upstream_api_key=CASE WHEN $18 THEN $19 ELSE upstream_api_key END,available_models=$20,test_model=$21,status_statistics_enabled=$22,auto_disable_allowed=$23,supports_websocket=$24 WHERE id=$1 AND updated_at=$25 RETURNING updated_at").bind(id).bind(input.channel_group_id).bind(&input.api_format).bind(&input.name).bind(&input.base_url).bind(input.enabled).bind(input.weight).bind(input.billing_multiplier).bind(input.proxy_id).bind(input.config_template_id).bind(override_document_present).bind(&override_document).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms).bind(&input.upstream_auth_kind).bind(&input.upstream_auth_header_name).bind(credential_present).bind(input.upstream_api_key.flatten()).bind(&input.available_models).bind(&input.test_model).bind(input.status_statistics_enabled).bind(input.auto_disable_allowed).bind(input.supports_websocket).bind(expected_updated_at.expect("PUT version")).fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?
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

async fn channel_is_provider_managed(
    transaction: &mut Transaction<'_, Postgres>,
    channel_id: Uuid,
) -> Result<bool, RepositoryError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT g.connector_kind <> 'openai_compatible' \
         FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id \
         WHERE c.id=$1",
    )
    .bind(channel_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)
}

async fn channel_recover(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = channel_audit(transaction, id).await?;
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at || before["auto_disabled"].as_bool() != Some(true)
    {
        return Err(RepositoryError::Conflict);
    }
    let reason = "manually recovered by administrator".to_owned();
    let updated_at = sqlx::query_scalar(
        "UPDATE channels
         SET auto_disabled=false, auto_disabled_reason=NULL
         WHERE id=$1 AND updated_at=$2 AND auto_disabled
         RETURNING updated_at",
    )
    .bind(id)
    .bind(expected_updated_at)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id,
        object_type: "channel",
        action: "manual_recover",
        before_redacted: before,
        after_redacted: channel_audit(transaction, id).await?,
        created_secret: None,
        reason: Some(reason),
        updated_at,
        correlation_id: None,
    })
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
async fn proxy_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let before = proxy_audit(transaction, id).await?;
    let current_updated_at: DateTime<Utc> = serde_json::from_value(before["updated_at"].clone())
        .map_err(|_| RepositoryError::Validation)?;
    if current_updated_at != expected_updated_at {
        return Err(RepositoryError::Conflict);
    }
    let in_use = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM channels WHERE proxy_id=$1) \
             OR EXISTS(SELECT 1 FROM codex_oauth_flows WHERE proxy_id=$1)",
    )
    .bind(id)
    .fetch_one(&mut **transaction)
    .await?;
    if in_use {
        return Err(RepositoryError::ProxyInUse);
    }
    let deleted = sqlx::query("DELETE FROM proxies WHERE id=$1 AND updated_at=$2")
        .bind(id)
        .bind(expected_updated_at)
        .execute(&mut **transaction)
        .await?;
    if deleted.rows_affected() != 1 {
        return Err(RepositoryError::Conflict);
    }
    Ok(MutationResult {
        id,
        object_type: "proxy",
        action: "delete",
        before_redacted: before,
        after_redacted: json!({}),
        created_secret: None,
        reason: None,
        updated_at: expected_updated_at,
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

async fn system_settings_update(
    transaction: &mut Transaction<'_, Postgres>,
    input: SystemSettingsInput,
    expected_updated_at: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    validate_system_settings_input(&input)?;
    let value = serde_json::to_value(&input).expect("system settings serialize");
    let before = system_settings_audit(transaction).await?;
    let updated_at = sqlx::query_scalar(
        "UPDATE system_settings SET value=$2 \
         WHERE setting_key=$1 AND updated_at=$3 RETURNING updated_at",
    )
    .bind(FORWARDING_SETTINGS_KEY)
    .bind(&value)
    .bind(expected_updated_at)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(MutationResult {
        id: forwarding_settings_object_id(),
        object_type: "system_settings",
        action: "update",
        before_redacted: before,
        after_redacted: system_settings_audit(transaction).await?,
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

async fn system_settings_audit(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<Value, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT value FROM system_settings WHERE setting_key=$1 FOR UPDATE",
    )
    .bind(FORWARDING_SETTINGS_KEY)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    Ok(system_settings_audit_value(&value))
}

async fn system_settings_input_for_update(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<SystemSettingsInput, RepositoryError> {
    let value = sqlx::query_scalar::<_, Value>(
        "SELECT value FROM system_settings WHERE setting_key=$1 FOR UPDATE",
    )
    .bind(FORWARDING_SETTINGS_KEY)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let settings = serde_json::from_value(value).map_err(|_| RepositoryError::Validation)?;
    validate_system_settings_input(&settings)?;
    Ok(settings)
}

fn automatic_disable_matches(
    settings: &SystemSettingsInput,
    trigger: &AutomaticDisableTrigger,
) -> bool {
    if !settings.automatic_disable.enabled {
        return false;
    }
    match trigger {
        AutomaticDisableTrigger::HttpStatus(status) => settings
            .automatic_disable
            .error_status_codes
            .contains(status),
        AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => settings
            .automatic_disable
            .error_message_keywords
            .iter()
            .any(|candidate| candidate.trim().to_lowercase() == keyword.to_lowercase()),
    }
}

fn automatic_disable_reason(trigger: &AutomaticDisableTrigger) -> String {
    match trigger {
        AutomaticDisableTrigger::HttpStatus(status) => {
            format!("automatic disable: upstream HTTP status {status}")
        }
        AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => {
            format!("automatic disable: configured error keyword `{keyword}`")
        }
    }
}

fn system_settings_audit_value(value: &Value) -> Value {
    json!({
        "setting_key": FORWARDING_SETTINGS_KEY,
        "value": value,
    })
}

fn system_settings_view(
    record: SystemSettingsRecord,
) -> Result<SystemSettingsView, RepositoryError> {
    if record.setting_key != FORWARDING_SETTINGS_KEY {
        return Err(RepositoryError::Validation);
    }
    let settings: SystemSettingsInput =
        serde_json::from_value(record.value).map_err(|_| RepositoryError::Validation)?;
    validate_system_settings_input(&settings)?;
    Ok(SystemSettingsView {
        settings,
        updated_at: record.updated_at,
    })
}

fn validate_system_settings_input(input: &SystemSettingsInput) -> Result<(), RepositoryError> {
    let api_hosts = &input.api_hosts;
    let upstream = &input.upstream;
    let request_retry = &input.request_retry;
    let passive_health = &input.passive_health;
    let automatic_disable = &input.automatic_disable;
    let scheduled_testing = &input.scheduled_testing;
    let session_affinity = &input.session_affinity;
    let websocket = &input.websocket;
    if !valid_api_hosts(api_hosts)
        || upstream.connect_timeout_seconds == 0
        || upstream.response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.stream_idle_timeout_seconds == 0
        || request_retry.max_retries == 0
        || request_retry.max_retries > MAX_REQUEST_RETRIES
        || passive_health.connection_failure_threshold == 0
        || passive_health.cooldown_seconds == 0
        || automatic_disable
            .error_status_codes
            .iter()
            .any(|status| !(100..=599).contains(status))
        || automatic_disable
            .error_message_keywords
            .iter()
            .any(|keyword| keyword.trim().is_empty() || keyword.chars().count() > 200)
        || scheduled_testing.interval_minutes == 0
        || scheduled_testing.prompt.trim().is_empty()
        || scheduled_testing.prompt.chars().count() > 4_000
        || !matches!(scheduled_testing.mode.as_str(), "global" | "failure_only")
        || !valid_session_affinity_input(session_affinity)
        || websocket.max_idle_connections > 4_096
        || websocket.idle_timeout_seconds == 0
        || websocket.idle_timeout_seconds > 3_600
        || websocket.max_connection_age_seconds < 60
        || websocket.max_connection_age_seconds > 3_600
        || websocket.idle_timeout_seconds >= websocket.max_connection_age_seconds
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

pub fn valid_api_hosts(api_hosts: &[String]) -> bool {
    if api_hosts.len() > 32 {
        return false;
    }
    let mut unique_hosts = HashSet::new();
    api_hosts.iter().all(|api_host| {
        if api_host.trim() != api_host || api_host.is_empty() || api_host.chars().count() > 2_048 {
            return false;
        }
        let Ok(url) = reqwest::Url::parse(api_host) else {
            return false;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return false;
        }
        unique_hosts.insert(url.to_string())
    })
}

fn valid_session_affinity_input(input: &SystemSessionAffinitySettingsInput) -> bool {
    if input.max_entries == 0
        || input.max_entries > 1_000_000
        || input.default_ttl_seconds == 0
        || input.default_ttl_seconds > 7 * 24 * 60 * 60
        || input.rules.len() > 64
    {
        return false;
    }
    let mut names = HashSet::new();
    input.rules.iter().all(|rule| {
        let name = rule.name.trim();
        name.chars().count() <= 64
            && !name.is_empty()
            && names.insert(name.to_lowercase())
            && !rule.api_formats.is_empty()
            && rule.api_formats.iter().collect::<HashSet<_>>().len() == rule.api_formats.len()
            && rule.api_formats.iter().all(|format| {
                matches!(
                    format.as_str(),
                    "open_ai_chat_completions" | "open_ai_responses"
                )
            })
            && rule.model_regex.len() <= 8
            && rule.model_regex.iter().collect::<HashSet<_>>().len() == rule.model_regex.len()
            && rule
                .model_regex
                .iter()
                .all(|pattern| valid_session_affinity_regex(pattern))
            && rule
                .value_regex
                .as_deref()
                .is_none_or(valid_session_affinity_regex)
            && !rule.key_sources.is_empty()
            && rule.key_sources.len() <= 8
            && rule
                .key_sources
                .iter()
                .all(valid_session_affinity_key_source)
            && rule
                .ttl_seconds
                .is_none_or(|ttl| ttl > 0 && ttl <= 7 * 24 * 60 * 60)
    })
}

fn valid_session_affinity_regex(pattern: &str) -> bool {
    !pattern.is_empty() && pattern.chars().count() <= 256 && Regex::new(pattern).is_ok()
}

fn valid_session_affinity_key_source(source: &SystemSessionAffinityKeySourceInput) -> bool {
    match source {
        SystemSessionAffinityKeySourceInput::RequestHeader { name } => {
            HeaderName::from_bytes(name.trim().as_bytes()).is_ok_and(|name| {
                !matches!(
                    name.as_str(),
                    "authorization"
                        | "proxy-authorization"
                        | "proxy-authenticate"
                        | "cookie"
                        | "set-cookie"
                        | "host"
                        | "content-length"
                        | "connection"
                        | "transfer-encoding"
                        | "keep-alive"
                        | "te"
                        | "trailer"
                        | "upgrade"
                        | "proxy-connection"
                )
            })
        }
        SystemSessionAffinityKeySourceInput::JsonPointer { pointer } => {
            let pointer = pointer.trim();
            pointer.chars().count() <= 256 && valid_session_affinity_json_pointer(pointer)
        }
    }
}

fn valid_session_affinity_json_pointer(pointer: &str) -> bool {
    if pointer.is_empty() || !pointer.starts_with('/') {
        return false;
    }
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            index += 1;
            if index >= bytes.len() || !matches!(bytes[index], b'0' | b'1') {
                return false;
            }
        }
        index += 1;
    }
    true
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
    #[error("the built-in user group is protected")]
    ProtectedUserGroup,
    #[error("the user group still has members")]
    UserGroupInUse,
    #[error("the proxy is still assigned to a channel or pending OAuth flow")]
    ProxyInUse,
    #[error("an administrator cannot delete their own account")]
    CannotDeleteSelf,
    #[error("the last active administrator cannot be deleted")]
    LastAdministrator,
    #[error("an administrator cannot disable their own account in a batch")]
    CannotDisableSelf,
    #[error("the user has no default API key policy")]
    DefaultApiKeyPolicyRequired,
    #[error("the user's default API key policy is disabled")]
    DefaultApiKeyPolicyDisabled,
    #[error("the selected API key target is not allowed by the user's policy")]
    ApiKeyTargetNotAllowed,
    #[error("the registration invitation code name or secret already exists")]
    RegistrationInvitationCodeConflict,
}
