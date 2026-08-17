//! Backend-neutral runtime snapshot records and system-setting contracts.

use std::{collections::HashSet, fmt};

use chrono::{DateTime, Utc};
use regex::Regex;
use reqwest::header::HeaderName;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::error::RepositoryError;
use crate::domain::{
    DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS, DEFAULT_MCP_IMAGE_REQUEST_BODY_BYTES,
    DEFAULT_MCP_IMAGE_RESULT_BYTES, DEFAULT_MCP_REQUEST_BODY_BYTES,
    DEFAULT_MCP_SEARCH_RESULT_BYTES, DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS,
    MAX_MCP_IMAGE_BYTES, MAX_REQUEST_RETRIES,
};

/// Singleton row that supplies database-backed process-wide runtime settings.
pub const FORWARDING_SETTINGS_KEY: &str = "forwarding_policy";
pub const SYSTEM_PROBE_USER_ID: Uuid = Uuid::from_u128(0x2c2e_3fd5_07e6_4c44_b5c7_cfe4_7bda_2b10);
pub const SYSTEM_PROBE_API_KEY_ID: Uuid =
    Uuid::from_u128(0x729d_37d8_2ad3_44ef_9e65_bf7e_410b_0f2f);
pub const DEFAULT_USER_GROUP_ID: Uuid = Uuid::from_u128(0x101);
pub const DEFAULT_ADMIN_GROUP_ID: Uuid = Uuid::from_u128(0x102);

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
    pub mcp_servers: Vec<McpServerRecord>,
}

/// Coherent database input for one complete runtime snapshot.
#[derive(Debug)]
pub struct RuntimeConfigRecords {
    pub control_plane: ControlPlaneRecords,
    pub system_settings: SystemSettingsRecord,
}

#[derive(Debug)]
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
    #[serde(default)]
    pub codex: SystemCodexSettingsInput,
    #[serde(default)]
    pub mcp: SystemMcpSettingsInput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemUpstreamSettingsInput {
    pub connect_timeout_seconds: u64,
    pub response_header_timeout_seconds: u64,
    #[serde(default = "default_images_response_header_timeout_seconds")]
    pub images_response_header_timeout_seconds: u64,
    #[serde(default = "default_standalone_web_search_response_header_timeout_seconds")]
    pub standalone_web_search_response_header_timeout_seconds: u64,
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
pub struct SystemCodexSettingsInput {
    #[serde(default = "default_codex_workspace_path")]
    pub workspace_path: String,
    #[serde(default = "default_codex_git_remote_url")]
    pub git_remote_url: String,
}

impl Default for SystemCodexSettingsInput {
    fn default() -> Self {
        Self {
            workspace_path: default_codex_workspace_path(),
            git_remote_url: default_codex_git_remote_url(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SystemMcpSettingsInput {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub allow_legacy_2025_11_25: bool,
    #[serde(default = "default_mcp_request_body_bytes")]
    pub request_body_bytes: usize,
    #[serde(default = "default_mcp_image_request_body_bytes")]
    pub image_request_body_bytes: usize,
    #[serde(default = "default_mcp_search_result_bytes")]
    pub search_result_bytes: usize,
    #[serde(default = "default_mcp_image_result_bytes")]
    pub image_result_bytes: usize,
}

impl Default for SystemMcpSettingsInput {
    fn default() -> Self {
        Self {
            enabled: false,
            public_base_url: None,
            allowed_origins: Vec::new(),
            allow_legacy_2025_11_25: false,
            request_body_bytes: default_mcp_request_body_bytes(),
            image_request_body_bytes: default_mcp_image_request_body_bytes(),
            search_result_bytes: default_mcp_search_result_bytes(),
            image_result_bytes: default_mcp_image_result_bytes(),
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

const fn default_images_response_header_timeout_seconds() -> u64 {
    DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS
}
const fn default_standalone_web_search_response_header_timeout_seconds() -> u64 {
    DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS
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

fn default_codex_workspace_path() -> String {
    crate::domain::DEFAULT_CODEX_WORKSPACE_PATH.into()
}

fn default_codex_git_remote_url() -> String {
    crate::domain::DEFAULT_CODEX_GIT_REMOTE_URL.into()
}

const fn default_mcp_request_body_bytes() -> usize {
    DEFAULT_MCP_REQUEST_BODY_BYTES
}

const fn default_mcp_image_request_body_bytes() -> usize {
    DEFAULT_MCP_IMAGE_REQUEST_BODY_BYTES
}

const fn default_mcp_search_result_bytes() -> usize {
    DEFAULT_MCP_SEARCH_RESULT_BYTES
}

const fn default_mcp_image_result_bytes() -> usize {
    DEFAULT_MCP_IMAGE_RESULT_BYTES
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

#[derive(Clone, Debug, Serialize)]
pub struct UserSettingsView {
    pub websocket_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserSettingsInput {
    pub websocket_enabled: bool,
}

pub struct ApiKeyRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub user_status: String,
    pub user_websocket_enabled: bool,
    pub user_filter_fast_mode: bool,
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
            .field("user_filter_fast_mode", &self.user_filter_fast_mode)
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
#[derive(Clone, Debug)]
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
#[derive(Debug)]
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
#[derive(Debug)]
pub struct ChannelGroupRecord {
    pub id: Uuid,
    pub name: String,
    pub api_format: String,
    pub connector_kind: String,
    pub request_compression: String,
    pub priority: i32,
    pub selection_strategy: String,
    pub enabled: bool,
}
#[derive(Clone)]
pub struct ChannelRecord {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub supports_websocket: bool,
    pub supports_standalone_web_search: bool,
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
            .field(
                "supports_standalone_web_search",
                &self.supports_standalone_web_search,
            )
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

#[derive(Debug)]
pub struct McpServerRecord {
    pub id: Uuid,
    pub slug: String,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub model_rule_id: Uuid,
    pub settings_version: i16,
    pub settings: Value,
    pub enabled: bool,
}

pub(super) fn validate_system_settings_input(
    input: &SystemSettingsInput,
) -> Result<(), RepositoryError> {
    let api_hosts = &input.api_hosts;
    let upstream = &input.upstream;
    let request_retry = &input.request_retry;
    let passive_health = &input.passive_health;
    let automatic_disable = &input.automatic_disable;
    let scheduled_testing = &input.scheduled_testing;
    let session_affinity = &input.session_affinity;
    let websocket = &input.websocket;
    let codex = &input.codex;
    let mcp = &input.mcp;
    if !valid_api_hosts(api_hosts)
        || upstream.connect_timeout_seconds == 0
        || upstream.response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.images_response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.standalone_web_search_response_header_timeout_seconds
            <= upstream.connect_timeout_seconds
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
        || !valid_codex_settings_input(codex)
        || !valid_mcp_settings_input(mcp)
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

pub fn valid_codex_settings_input(input: &SystemCodexSettingsInput) -> bool {
    let workspace_path = input.workspace_path.as_str();
    if workspace_path.trim() != workspace_path
        || workspace_path.is_empty()
        || workspace_path.chars().count() > 1_024
        || !workspace_path.starts_with('/')
        || workspace_path.chars().any(char::is_control)
    {
        return false;
    }
    let git_remote_url = input.git_remote_url.as_str();
    if git_remote_url.trim() != git_remote_url
        || git_remote_url.is_empty()
        || git_remote_url.chars().count() > 2_048
    {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(git_remote_url) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.path() != "/"
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn valid_mcp_settings_input(input: &SystemMcpSettingsInput) -> bool {
    if input.request_body_bytes == 0
        || input.image_request_body_bytes == 0
        || input.search_result_bytes == 0
        || input.image_result_bytes == 0
        || input.image_request_body_bytes > MAX_MCP_IMAGE_BYTES
        || input.image_result_bytes > MAX_MCP_IMAGE_BYTES
        || input.allowed_origins.len() > 64
        || (input.enabled && input.public_base_url.is_none())
    {
        return false;
    }
    if input
        .public_base_url
        .as_deref()
        .is_some_and(|value| canonical_mcp_origin(value).is_none())
    {
        return false;
    }
    let mut origins = HashSet::new();
    input
        .allowed_origins
        .iter()
        .all(|origin| canonical_mcp_origin(origin).is_some_and(|origin| origins.insert(origin)))
}

fn canonical_mcp_origin(value: &str) -> Option<String> {
    if value.trim() != value || value.is_empty() || value.chars().count() > 2_048 || value == "*" {
        return None;
    }
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
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
