//! Bootstrap TOML validation and database control-plane snapshot compilation.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::ArcSwap;
use regex::Regex;
use reqwest::{
    Url,
    header::{HeaderName, HeaderValue},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnectOptions;
use thiserror::Error;
use tokio::sync::watch;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    domain::{
        AdvancedBilling, ApiFormat, ApiKeyHash, ApiKeyPermission, AuthorizationProfile,
        AutomaticDisableSettings, ChannelTimeoutPolicy, CompiledApiKey, CompiledCandidate,
        CompiledChannel, CompiledChannelGroup, CompiledChannelUpstreamPolicy,
        CompiledConfigTemplate, CompiledMcpServer, CompiledModelRule, CompiledProxy,
        CompiledRouteTier, CompiledRuntimeConfig, CompiledScheduledTestModel, ConnectorKind,
        DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS, DEFAULT_MCP_IMAGE_REQUEST_BODY_BYTES,
        DEFAULT_MCP_IMAGE_RESULT_BYTES, DEFAULT_MCP_REQUEST_BODY_BYTES,
        DEFAULT_MCP_SEARCH_RESULT_BYTES,
        DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS, ImageMcpSettings,
        MAX_MCP_IMAGE_BYTES, MAX_REQUEST_RETRIES, McpServerKind, McpTransportSettings,
        ModelPriceSnapshot, ModelRouteKey, NoProxyHost, PassiveHealthSettings,
        RequestRetrySettings, ResponsesWebSocketSettings, ScheduledTestingMode,
        ScheduledTestingSettings, SelectionStrategy, SessionAffinityKeySource, SessionAffinityRule,
        SessionAffinitySettings, SystemRuntimeSettings, UpstreamAuth, UpstreamTimeoutDefaults,
        WebSearchMcpSettings,
    },
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        FORWARDING_SETTINGS_KEY, McpServerRecord, ModelRecord, ModelRuleRecord, ProxyRecord,
        RuntimeConfigRecords, SystemMcpSettingsInput, SystemSessionAffinityKeySourceInput,
        SystemSessionAffinityRuleInput, SystemSessionAffinitySettingsInput, SystemSettingsInput,
        SystemSettingsRecord, valid_api_hosts,
    },
    request_policy::{client_header_allowed, client_header_explicitly_ignored},
    transforms::{TransformCompileError, TransformPlan, compile_document, declared_api_format},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub request_retry: RequestRetryConfig,
    pub runtime_config: RuntimeConfigSettings,
    #[serde(default)]
    pub request_logging: RequestLoggingConfig,
    #[serde(default)]
    pub passive_health: PassiveHealthConfig,
    #[serde(default)]
    pub automatic_disable: AutomaticDisableConfig,
    #[serde(default)]
    pub scheduled_testing: ScheduledTestingConfig,
    #[serde(default)]
    pub session_affinity: SessionAffinityConfig,
    #[serde(default)]
    pub models_sync: ModelsSyncConfig,
    #[serde(default)]
    pub request_limits: RequestLimitsFileConfig,
    #[serde(default)]
    pub mcp: McpFileConfig,
    #[serde(default)]
    pub console: ConsoleFileConfig,
    #[serde(default)]
    pub auth: AuthFileConfig,
    pub observability: ObservabilityConfig,
}
impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents =
            Zeroizing::new(
                fs::read_to_string(path).map_err(|source| ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                })?,
            );
        toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            line: source
                .span()
                .and_then(|span| contents[..span.start].lines().count().checked_add(1)),
            column: source.span().and_then(|span| {
                contents[..span.start]
                    .rsplit_once('\n')
                    .map_or(Some(span.start + 1), |(_, line)| line.len().checked_add(1))
            }),
        })
    }
    pub fn validate(self) -> Result<BootstrapConfig, ConfigError> {
        validate_server(&self.server)?;
        validate_database(&self.database)?;
        validate_upstream(&self.upstream)?;
        validate_request_retry_config(&self.request_retry)?;
        validate_models_sync(&self.models_sync)?;
        let request_limits = RequestLimitsConfig::resolve(self.request_limits)?;
        let mcp = validate_mcp(self.mcp)?;
        let console = validate_console(self.console, self.auth)?;
        if self.runtime_config.reload_interval_seconds == 0 {
            return Err(ConfigError::Compile(
                "runtime_config reload_interval_seconds must be greater than zero".into(),
            ));
        }
        if self.request_logging.queue_capacity == 0
            || self.request_logging.database_max_connections == 0
            || self.request_logging.ingest_batch_size == 0
            || self.request_logging.ingest_batch_size > 10_000
            || self.request_logging.projection_batch_size == 0
            || self.request_logging.projection_batch_size > 10_000
            || self.request_logging.settlement_batch_size <= 0
            || self.request_logging.settlement_batch_size > 10_000
            || self.request_logging.settlement_interval_milliseconds == 0
            || self.request_logging.spool_sync_interval_milliseconds == 0
            || self.request_logging.spool_compaction_threshold_bytes == 0
            || self.request_logging.metrics_interval_seconds == 0
            || self.request_logging.shutdown_drain_seconds == 0
            || self.request_logging.spool_directory.as_os_str().is_empty()
        {
            return Err(ConfigError::Compile(
                "request_logging limits and spool_directory must be nonzero and nonempty".into(),
            ));
        }
        if self.passive_health.connection_failure_threshold == 0
            || self.passive_health.cooldown_seconds == 0
        {
            return Err(ConfigError::Compile(
                "passive_health threshold and cooldown must be greater than zero".into(),
            ));
        }
        validate_automatic_disable_config(&self.automatic_disable)?;
        validate_scheduled_testing_config(&self.scheduled_testing)?;
        validate_session_affinity_config(&self.session_affinity)?;
        require("observability filter", &self.observability.filter)?;
        Ok(BootstrapConfig {
            server: self.server,
            database: self.database,
            upstream: self.upstream,
            request_retry: self.request_retry,
            runtime_config: self.runtime_config,
            request_logging: self.request_logging,
            passive_health: self.passive_health,
            automatic_disable: self.automatic_disable,
            scheduled_testing: self.scheduled_testing,
            session_affinity: self.session_affinity,
            models_sync: self.models_sync,
            request_limits,
            mcp,
            console,
            observability: self.observability,
        })
    }
}

pub struct BootstrapConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub request_retry: RequestRetryConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub request_logging: RequestLoggingConfig,
    pub passive_health: PassiveHealthConfig,
    pub automatic_disable: AutomaticDisableConfig,
    pub scheduled_testing: ScheduledTestingConfig,
    pub session_affinity: SessionAffinityConfig,
    pub models_sync: ModelsSyncConfig,
    pub request_limits: RequestLimitsConfig,
    pub mcp: SystemMcpSettingsInput,
    pub console: Option<ConsoleListenerConfig>,
    pub observability: ObservabilityConfig,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_shutdown_grace_period_seconds")]
    pub shutdown_grace_period_seconds: u64,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    /// Preferred production password source. When set, the URL must not also
    /// contain a password.
    #[serde(default)]
    pub password_file: Option<PathBuf>,
    pub max_connections: u32,
    pub connect_timeout_seconds: u64,
}
impl DatabaseConfig {
    pub fn connect_options(&self) -> Result<PgConnectOptions, ConfigError> {
        let mut options = self
            .url
            .parse::<PgConnectOptions>()
            .map_err(|_| ConfigError::Compile("database URL is invalid".into()))?;
        let Some(path) = self.password_file.as_ref() else {
            return Ok(options);
        };
        let mut password = Zeroizing::new(fs::read_to_string(path).map_err(|source| {
            ConfigError::DatabasePasswordRead {
                path: path.clone(),
                source,
            }
        })?);
        while matches!(password.as_bytes().last(), Some(b'\n' | b'\r')) {
            password.pop();
        }
        if password.is_empty() {
            return Err(ConfigError::Compile(
                "database password_file must not be empty".into(),
            ));
        }
        options = options.password(password.as_str());
        Ok(options)
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
/// One-time bootstrap source for the database-backed forwarding timeout
/// policy. It is inserted only while the `system_settings` row is absent.
pub struct UpstreamConfig {
    pub connect_timeout_seconds: u64,
    pub response_header_timeout_seconds: u64,
    #[serde(default = "default_images_response_header_timeout_seconds")]
    pub images_response_header_timeout_seconds: u64,
    #[serde(default = "default_standalone_web_search_response_header_timeout_seconds")]
    pub standalone_web_search_response_header_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
}
/// One-time bootstrap source for pre-header request failover.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestRetryConfig {
    #[serde(default = "default_request_retry_enabled")]
    pub enabled: bool,
    #[serde(default = "default_request_retry_max_retries")]
    pub max_retries: u32,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigSettings {
    pub reload_interval_seconds: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestLoggingConfig {
    #[serde(default = "default_request_log_queue_capacity")]
    pub queue_capacity: usize,
    #[serde(default = "default_request_log_database_max_connections")]
    pub database_max_connections: u32,
    #[serde(default = "default_request_log_ingest_batch_size")]
    pub ingest_batch_size: usize,
    #[serde(default = "default_request_log_projection_batch_size")]
    pub projection_batch_size: usize,
    #[serde(default = "default_request_log_settlement_batch_size")]
    pub settlement_batch_size: i64,
    #[serde(default = "default_request_log_settlement_interval_milliseconds")]
    pub settlement_interval_milliseconds: u64,
    #[serde(default = "default_request_log_spool_directory")]
    pub spool_directory: PathBuf,
    #[serde(default = "default_request_log_spool_sync_interval_milliseconds")]
    pub spool_sync_interval_milliseconds: u64,
    #[serde(default = "default_request_log_spool_compaction_threshold_bytes")]
    pub spool_compaction_threshold_bytes: u64,
    #[serde(default = "default_request_log_metrics_interval_seconds")]
    pub metrics_interval_seconds: u64,
    #[serde(default = "default_request_log_shutdown_drain_seconds")]
    pub shutdown_drain_seconds: u64,
}
/// One-time bootstrap source for database-backed passive connectivity policy.
/// Defaults are three connection failures and a 30 second cooldown.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassiveHealthConfig {
    #[serde(default = "default_connection_failure_threshold")]
    pub connection_failure_threshold: u32,
    #[serde(default = "default_passive_health_cooldown_seconds")]
    pub cooldown_seconds: u64,
}
/// Bounds explicit administrator-triggered catalog fetches. This is process
/// configuration rather than control-plane state, so changing it requires a
/// restart just like the database connection settings.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelsSyncConfig {
    #[serde(default = "default_models_dev_api_url")]
    pub api_url: String,
    #[serde(default = "default_models_sync_timeout_seconds")]
    pub request_timeout_seconds: u64,
    #[serde(default = "default_models_sync_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_models_sync_max_metadata_bytes")]
    pub max_model_metadata_bytes: usize,
    #[serde(default = "default_models_sync_max_selections")]
    pub max_selections: usize,
}
impl Default for ModelsSyncConfig {
    fn default() -> Self {
        Self {
            api_url: default_models_dev_api_url(),
            request_timeout_seconds: default_models_sync_timeout_seconds(),
            max_response_bytes: default_models_sync_max_response_bytes(),
            max_model_metadata_bytes: default_models_sync_max_metadata_bytes(),
            max_selections: default_models_sync_max_selections(),
        }
    }
}
impl Default for PassiveHealthConfig {
    fn default() -> Self {
        Self {
            connection_failure_threshold: default_connection_failure_threshold(),
            cooldown_seconds: default_passive_health_cooldown_seconds(),
        }
    }
}

impl Default for RequestRetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_request_retry_enabled(),
            max_retries: default_request_retry_max_retries(),
        }
    }
}

/// One-time bootstrap source for database-backed automatic-disable policy.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomaticDisableConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub error_status_codes: Vec<u16>,
    #[serde(default)]
    pub error_message_keywords: Vec<String>,
}

/// One-time bootstrap source for database-backed periodic channel tests.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledTestingConfig {
    #[serde(default = "default_scheduled_testing_mode")]
    pub mode: String,
    #[serde(default = "default_scheduled_testing_auto_recover")]
    pub auto_recover: bool,
    #[serde(default = "default_scheduled_testing_interval_minutes")]
    pub interval_minutes: u64,
    #[serde(default = "default_scheduled_testing_prompt")]
    pub prompt: String,
}

/// One-time bootstrap source for database-backed session-affinity rules.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAffinityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_session_affinity_max_entries")]
    pub max_entries: usize,
    #[serde(default = "default_session_affinity_ttl_seconds")]
    pub default_ttl_seconds: u64,
    #[serde(default)]
    pub rules: Vec<SystemSessionAffinityRuleInput>,
}

impl Default for SessionAffinityConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_entries: default_session_affinity_max_entries(),
            default_ttl_seconds: default_session_affinity_ttl_seconds(),
            rules: Vec::new(),
        }
    }
}

impl Default for ScheduledTestingConfig {
    fn default() -> Self {
        Self {
            mode: default_scheduled_testing_mode(),
            auto_recover: default_scheduled_testing_auto_recover(),
            interval_minutes: default_scheduled_testing_interval_minutes(),
            prompt: default_scheduled_testing_prompt(),
        }
    }
}
impl Default for RequestLoggingConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_request_log_queue_capacity(),
            database_max_connections: default_request_log_database_max_connections(),
            ingest_batch_size: default_request_log_ingest_batch_size(),
            projection_batch_size: default_request_log_projection_batch_size(),
            settlement_batch_size: default_request_log_settlement_batch_size(),
            settlement_interval_milliseconds: default_request_log_settlement_interval_milliseconds(
            ),
            spool_directory: default_request_log_spool_directory(),
            spool_sync_interval_milliseconds: default_request_log_spool_sync_interval_milliseconds(
            ),
            spool_compaction_threshold_bytes: default_request_log_spool_compaction_threshold_bytes(
            ),
            metrics_interval_seconds: default_request_log_metrics_interval_seconds(),
            shutdown_drain_seconds: default_request_log_shutdown_drain_seconds(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    pub filter: String,
}

/// File-only request-size settings. The public proxy and authenticated Console
/// traffic intentionally use independent limits because their payload shapes
/// and abuse profiles differ.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestLimitsFileConfig {
    #[serde(default)]
    pub proxy_body_bytes: Option<usize>,
    #[serde(default = "default_image_edit_body_bytes")]
    pub image_edit_body_bytes: usize,
    #[serde(default = "default_image_edit_file_bytes")]
    pub image_edit_file_bytes: usize,
    #[serde(default = "default_image_edit_memory_bytes")]
    pub image_edit_memory_bytes: usize,
    #[serde(default = "default_image_edit_spool_directory")]
    pub image_edit_spool_directory: PathBuf,
    #[serde(default = "default_console_body_bytes")]
    pub console_body_bytes: usize,
    #[serde(default = "default_auth_body_bytes")]
    pub auth_body_bytes: usize,
}
impl Default for RequestLimitsFileConfig {
    fn default() -> Self {
        Self {
            proxy_body_bytes: None,
            image_edit_body_bytes: default_image_edit_body_bytes(),
            image_edit_file_bytes: default_image_edit_file_bytes(),
            image_edit_memory_bytes: default_image_edit_memory_bytes(),
            image_edit_spool_directory: default_image_edit_spool_directory(),
            console_body_bytes: default_console_body_bytes(),
            auth_body_bytes: default_auth_body_bytes(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RequestLimitsConfig {
    pub proxy_body_bytes: usize,
    pub image_edit_body_bytes: usize,
    pub image_edit_file_bytes: usize,
    pub image_edit_memory_bytes: usize,
    pub image_edit_spool_directory: PathBuf,
    pub console_body_bytes: usize,
    pub auth_body_bytes: usize,
}
impl RequestLimitsConfig {
    fn resolve(file: RequestLimitsFileConfig) -> Result<Self, ConfigError> {
        let proxy_body_bytes = file
            .proxy_body_bytes
            .unwrap_or_else(default_proxy_body_bytes);
        if proxy_body_bytes == 0
            || file.image_edit_body_bytes == 0
            || file.image_edit_file_bytes == 0
            || file.image_edit_memory_bytes == 0
            || file.console_body_bytes == 0
            || file.auth_body_bytes == 0
            || file.image_edit_spool_directory.as_os_str().is_empty()
        {
            return Err(ConfigError::Compile(
                "request body limits must be greater than zero and the Images edit spool directory must be nonempty"
                    .into(),
            ));
        }
        if file.image_edit_memory_bytes > file.image_edit_body_bytes {
            return Err(ConfigError::Compile(
                "image_edit_memory_bytes must not exceed image_edit_body_bytes".into(),
            ));
        }
        if file.image_edit_file_bytes > file.image_edit_body_bytes {
            return Err(ConfigError::Compile(
                "image_edit_file_bytes must not exceed image_edit_body_bytes".into(),
            ));
        }
        Ok(Self {
            proxy_body_bytes,
            image_edit_body_bytes: file.image_edit_body_bytes,
            image_edit_file_bytes: file.image_edit_file_bytes,
            image_edit_memory_bytes: file.image_edit_memory_bytes,
            image_edit_spool_directory: file.image_edit_spool_directory,
            console_body_bytes: file.console_body_bytes,
            auth_body_bytes: file.auth_body_bytes,
        })
    }
}

/// The separate browser/control-plane listener. It is deliberately named
/// Console rather than Admin: administrator is a user role, not an API type.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleFileConfig {
    #[serde(default)]
    pub enabled: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Serves the embedded Console web UI from this listener. Requires the
    /// `embedded-console-ui` cargo feature and a populated `web/console/dist`.
    #[serde(default)]
    pub ui_enabled: bool,
}

/// One-time bootstrap source for database-backed MCP transport settings.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpFileConfig {
    #[serde(default)]
    pub enabled: bool,
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

impl Default for McpFileConfig {
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

/// File-only JWT setup. Private key material remains in a separate protected
/// file, never in TOML.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthFileConfig {
    pub issuer: Option<String>,
    pub audience: Option<String>,
    pub access_token_ttl_seconds: Option<u64>,
    pub refresh_token_ttl_seconds: Option<u64>,
    pub key_id: Option<String>,
    pub signing_key_path: Option<PathBuf>,
    pub verification_key_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct ConsoleListenerConfig {
    pub address: SocketAddr,
    pub allowed_origins: Vec<String>,
    pub auth: AuthConfig,
    pub ui_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuthConfig {
    pub issuer: String,
    pub audience: String,
    pub access_token_ttl_seconds: u64,
    pub refresh_token_ttl_seconds: u64,
    pub key_id: String,
    pub signing_key_path: PathBuf,
    pub verification_key_path: PathBuf,
}

const fn default_shutdown_grace_period_seconds() -> u64 {
    60
}
const fn default_proxy_body_bytes() -> usize {
    1_048_576
}
const fn default_image_edit_body_bytes() -> usize {
    64 * 1_024 * 1_024
}
const fn default_image_edit_file_bytes() -> usize {
    50 * 1_024 * 1_024
}
const fn default_image_edit_memory_bytes() -> usize {
    1_048_576
}
fn default_image_edit_spool_directory() -> PathBuf {
    PathBuf::from("./data/image-edit-spool")
}
const fn default_console_body_bytes() -> usize {
    262_144
}
const fn default_auth_body_bytes() -> usize {
    16_384
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
const fn default_request_log_queue_capacity() -> usize {
    1_024
}
const fn default_request_log_database_max_connections() -> u32 {
    4
}
const fn default_request_log_ingest_batch_size() -> usize {
    4_096
}
const fn default_request_log_projection_batch_size() -> usize {
    2_048
}
const fn default_request_log_settlement_batch_size() -> i64 {
    4_096
}
const fn default_request_log_settlement_interval_milliseconds() -> u64 {
    500
}
fn default_request_log_spool_directory() -> PathBuf {
    PathBuf::from("./data/request-log-spool")
}
const fn default_request_log_spool_sync_interval_milliseconds() -> u64 {
    10
}
const fn default_request_log_spool_compaction_threshold_bytes() -> u64 {
    256 * 1_024 * 1_024
}
const fn default_request_log_metrics_interval_seconds() -> u64 {
    10
}
const fn default_request_log_shutdown_drain_seconds() -> u64 {
    60
}
const fn default_connection_failure_threshold() -> u32 {
    3
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
const fn default_passive_health_cooldown_seconds() -> u64 {
    30
}
fn default_scheduled_testing_mode() -> String {
    "global".into()
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
const fn default_session_affinity_max_entries() -> usize {
    100_000
}
const fn default_session_affinity_ttl_seconds() -> u64 {
    3_600
}
fn default_models_dev_api_url() -> String {
    "https://models.dev/api.json".into()
}
const fn default_models_sync_timeout_seconds() -> u64 {
    15
}
const fn default_models_sync_max_response_bytes() -> usize {
    10 * 1_024 * 1_024
}
const fn default_models_sync_max_metadata_bytes() -> usize {
    64 * 1_024
}
const fn default_models_sync_max_selections() -> usize {
    100
}

pub struct RuntimeConfig {
    current: ArcSwap<CompiledRuntimeConfig>,
    updates: watch::Sender<Arc<CompiledRuntimeConfig>>,
}
impl RuntimeConfig {
    #[must_use]
    pub fn new(initial: CompiledRuntimeConfig) -> Self {
        let initial = Arc::new(initial);
        let (updates, _) = watch::channel(Arc::clone(&initial));
        Self {
            current: ArcSwap::from(initial),
            updates,
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> Arc<CompiledRuntimeConfig> {
        self.current.load_full()
    }
    pub fn replace_snapshot(&self, next: Arc<CompiledRuntimeConfig>) {
        self.current.store(Arc::clone(&next));
        self.updates.send_replace(next);
    }

    #[cfg(feature = "mcp-server")]
    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<CompiledRuntimeConfig>> {
        self.updates.subscribe()
    }
}

/// Compiles database control-plane resources with the compatibility defaults
/// used by direct unit fixtures. Production snapshots use
/// [`compile_runtime_config`] so persisted system settings are included.
pub fn compile_control_plane(
    records: ControlPlaneRecords,
) -> Result<CompiledRuntimeConfig, ConfigError> {
    compile_control_plane_with_system_settings(records, SystemRuntimeSettings::default())
}

/// Compiles one complete database runtime snapshot, including the singleton
/// process-wide policy stored in `system_settings`.
pub fn compile_runtime_config(
    records: RuntimeConfigRecords,
) -> Result<CompiledRuntimeConfig, ConfigError> {
    let system_settings = compile_system_settings(records.system_settings)?;
    compile_control_plane_with_system_settings(records.control_plane, system_settings)
}

/// Compiles control-plane resources with an already validated system policy.
///
/// This remains public for deterministic tests that need non-default global
/// forwarding behavior without a PostgreSQL fixture.
pub fn compile_control_plane_with_system_settings(
    records: ControlPlaneRecords,
    system_settings: SystemRuntimeSettings,
) -> Result<CompiledRuntimeConfig, ConfigError> {
    let mut all_groups = HashMap::new();
    let mut groups = HashMap::new();
    for group in records.groups {
        validate_group(&group)?;
        insert_unique(&mut all_groups, group.id, group, "channel group")?;
    }
    for group in all_groups.values() {
        if group.enabled {
            groups.insert(
                group.id,
                Arc::new(CompiledChannelGroup::new_with_connector(
                    group.id,
                    parse_format(&group.api_format)?,
                    parse_connector_kind(&group.connector_kind)?,
                    group.priority,
                    parse_strategy(&group.selection_strategy)?,
                )),
            );
        }
    }
    let proxies = compile_proxies(records.proxies)?;
    let templates = compile_templates(records.templates)?;
    let models_by_source = index_models(records.models)?;
    let mut channels = HashMap::new();
    let mut probe_channels = HashMap::new();
    let mut all_channels = HashMap::new();
    let mut validated_channels = Vec::new();
    let mut channel_ids = HashSet::new();
    for channel in records.channels {
        if !channel_ids.insert(channel.id) {
            return Err(dup("channel id"));
        }
        validate_channel(&channel, &all_groups)?;
        validate_channel_resources(&channel, &proxies, &templates)?;
        all_channels.insert(channel.id, channel.clone());
        validated_channels.push(channel);
    }
    let mut sorted_channel_ids = all_channels.keys().copied().collect::<Vec<_>>();
    sorted_channel_ids.sort_unstable();
    let channel_slots = sorted_channel_ids
        .iter()
        .enumerate()
        .map(|(slot, id)| (*id, slot))
        .collect::<HashMap<_, _>>();
    let mut channels_by_group = HashMap::<Uuid, Vec<Uuid>>::new();
    let mut channels_by_group_model = HashMap::<Uuid, HashMap<String, Vec<Uuid>>>::new();
    for id in &sorted_channel_ids {
        let channel = &all_channels[id];
        channels_by_group
            .entry(channel.channel_group_id)
            .or_default()
            .push(*id);
        for model in &channel.available_models {
            channels_by_group_model
                .entry(channel.channel_group_id)
                .or_default()
                .entry(model.clone())
                .or_default()
                .push(*id);
        }
    }
    let scheduled_test_models = compile_scheduled_test_models(&all_channels, &models_by_source)?;
    for channel in validated_channels {
        if channel.enabled {
            let auth = compile_auth(&channel)?;
            let api_format = parse_format(&channel.api_format)?;
            let proxy = channel.proxy_id.map(|id| {
                proxies
                    .get(&id)
                    .cloned()
                    .expect("validated proxy reference")
            });
            let template = channel.config_template_id.map(|id| {
                templates
                    .get(&id)
                    .cloned()
                    .expect("validated template reference")
            });
            let channel_override = compile_channel_document(&channel, api_format)?;
            let defaults = template.as_ref().map_or_else(
                || TransformPlan::noop(api_format),
                |template| template.transform_plan(api_format).clone(),
            );
            let effective_transforms = TransformPlan::compose(&defaults, &channel_override)
                .map_err(transform_error("channel effective transform plan"))?;
            if channel.supports_standalone_web_search
                && !effective_transforms.request_json().is_empty()
            {
                return Err(ConfigError::Compile(
                    "standalone web search channels do not support request JSON transforms".into(),
                ));
            }
            let upstream_policy = CompiledChannelUpstreamPolicy::new_with_default_connect_timeout(
                proxy,
                template,
                channel_override,
                effective_transforms,
                compile_timeouts(&channel)?,
                system_settings.upstream_timeouts().connect(),
            );
            let connector_kind =
                parse_connector_kind(&all_groups[&channel.channel_group_id].connector_kind)?;
            let compiled = Arc::new(
                CompiledChannel::new_with_connector_policy_automation_and_billing(
                    channel.id,
                    channel.channel_group_id,
                    api_format,
                    connector_kind,
                    parse_url(channel.id, &channel.base_url)?,
                    channel.weight,
                    channel.billing_multiplier,
                    auth,
                    channel
                        .available_models
                        .iter()
                        .map(|model| Arc::<str>::from(model.as_str()))
                        .collect(),
                    channel.supports_websocket,
                    channel.supports_standalone_web_search,
                    channel.auto_disable_allowed,
                    channel.auto_disabled,
                    channel.test_model.as_deref().map(Arc::<str>::from),
                    upstream_policy,
                ),
            );
            probe_channels.insert(channel.id, Arc::clone(&compiled));
            if !channel.auto_disabled && all_groups[&channel.channel_group_id].enabled {
                channels.insert(channel.id, compiled);
            }
        }
    }
    let CompiledRules {
        model_rules,
        routes_by_channel_slot,
    } = compile_rules(
        records.model_rules,
        &all_groups,
        &all_channels,
        &channels_by_group_model,
        &channel_slots,
        &groups,
        &channels,
    )?;
    let api_keys = compile_keys(
        records.api_keys,
        &all_groups,
        &all_channels,
        &channels_by_group,
        &channel_slots,
        &model_rules,
        &routes_by_channel_slot,
    )?;
    let mcp_servers = compile_mcp_servers(
        records.mcp_servers,
        &model_rules,
        &all_channels,
        &channel_slots,
    )?;
    Ok(
        CompiledRuntimeConfig::with_resources_system_settings_and_probe_channels(
            api_keys,
            model_rules,
            channels,
            probe_channels,
            scheduled_test_models,
            groups,
            proxies,
            templates,
            system_settings,
        )
        .with_mcp_servers(mcp_servers),
    )
}

fn compile_mcp_servers(
    records: Vec<McpServerRecord>,
    model_rules: &HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    channels: &HashMap<Uuid, ChannelRecord>,
    channel_slots: &HashMap<Uuid, usize>,
) -> Result<HashMap<Arc<str>, Arc<CompiledMcpServer>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    let slug_pattern = Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$").expect("static MCP slug regex");
    let rules_by_id = model_rules
        .values()
        .map(|rule| (rule.id(), Arc::clone(rule)))
        .collect::<HashMap<_, _>>();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("MCP server id"));
        }
        require("MCP server slug", &record.slug)?;
        require("MCP server name", &record.name)?;
        if !slug_pattern.is_match(&record.slug)
            || record.name.len() > 100
            || record
                .description
                .as_ref()
                .is_some_and(|value| value.len() > 1_000)
        {
            return Err(ConfigError::Compile(
                "MCP server slug, name, or description is invalid".into(),
            ));
        }
        if record.settings_version != 1 {
            return Err(ConfigError::Compile(
                "unsupported MCP server settings version".into(),
            ));
        }
        let kind = McpServerKind::parse(&record.kind)
            .ok_or_else(|| ConfigError::Compile("unsupported MCP server kind".into()))?;
        let settings = match kind {
            McpServerKind::WebSearch => {
                let mut settings = serde_json::from_value::<WebSearchMcpSettings>(record.settings)
                    .map_err(|_| ConfigError::Compile("invalid web-search MCP settings".into()))?;
                validate_web_search_mcp_settings(&mut settings)?;
                ValidatedMcpSettings::WebSearch(settings)
            }
            McpServerKind::Image => {
                let mut settings = serde_json::from_value::<ImageMcpSettings>(record.settings)
                    .map_err(|_| ConfigError::Compile("invalid image MCP settings".into()))?;
                validate_image_mcp_settings(&mut settings)?;
                ValidatedMcpSettings::Image(settings)
            }
        };
        if !record.enabled {
            continue;
        }
        let model_rule = rules_by_id
            .get(&record.model_rule_id)
            .cloned()
            .ok_or_else(|| {
                ConfigError::Compile("enabled MCP server references a missing model rule".into())
            })?;
        let slug = Arc::<str>::from(record.slug);
        let name = Arc::<str>::from(record.name);
        let description = record.description.map(Arc::<str>::from);
        let compiled = match settings {
            ValidatedMcpSettings::WebSearch(settings) => {
                if model_rule.api_format() != ApiFormat::OpenAiResponses {
                    return Err(ConfigError::Compile(
                        "web-search MCP servers require an OpenAI Responses model rule".into(),
                    ));
                }
                if !channels.values().any(|channel| {
                    channel.supports_standalone_web_search
                        && channel_slots
                            .get(&channel.id)
                            .is_some_and(|slot| model_rule.has_configured_candidate(*slot))
                }) {
                    return Err(ConfigError::Compile(
                        "web-search MCP servers require a model rule with a search-capable channel"
                            .into(),
                    ));
                }
                CompiledMcpServer::new_web_search(
                    record.id,
                    Arc::clone(&slug),
                    name,
                    description,
                    model_rule,
                    settings,
                )
            }
            ValidatedMcpSettings::Image(settings) => {
                if model_rule.api_format() != ApiFormat::OpenAiImages {
                    return Err(ConfigError::Compile(
                        "image MCP servers require an OpenAI Images model rule".into(),
                    ));
                }
                if !channels.values().any(|channel| {
                    channel.api_format == "open_ai_images"
                        && channel_slots
                            .get(&channel.id)
                            .is_some_and(|slot| model_rule.has_configured_candidate(*slot))
                }) {
                    return Err(ConfigError::Compile(
                        "image MCP servers require a model rule with an Images channel".into(),
                    ));
                }
                CompiledMcpServer::new_image(
                    record.id,
                    Arc::clone(&slug),
                    name,
                    description,
                    model_rule,
                    settings,
                )
            }
        };
        let compiled = Arc::new(compiled);
        if result.insert(slug, compiled).is_some() {
            return Err(dup("enabled MCP server slug"));
        }
    }
    Ok(result)
}

enum ValidatedMcpSettings {
    WebSearch(WebSearchMcpSettings),
    Image(ImageMcpSettings),
}

fn validate_image_mcp_settings(settings: &mut ImageMcpSettings) -> Result<(), ConfigError> {
    if settings.size == "auto" {
        return Ok(());
    }
    let Some((width, height)) = settings.size.split_once('x') else {
        return Err(ConfigError::Compile(
            "image MCP size must be auto or WIDTHxHEIGHT".into(),
        ));
    };
    if !valid_image_dimension(width) || !valid_image_dimension(height) {
        return Err(ConfigError::Compile(
            "image MCP dimensions must be canonical integers from 64 to 8192".into(),
        ));
    }
    Ok(())
}

fn valid_image_dimension(value: &str) -> bool {
    !value.is_empty()
        && (value.len() == 1 || !value.starts_with('0'))
        && value
            .parse::<u32>()
            .is_ok_and(|dimension| (64..=8_192).contains(&dimension))
}

fn validate_web_search_mcp_settings(
    settings: &mut WebSearchMcpSettings,
) -> Result<(), ConfigError> {
    let limits = &settings.max_output_tokens;
    if limits.short == 0
        || limits.short > limits.medium
        || limits.medium > limits.long
        || limits.long > 100_000
    {
        return Err(ConfigError::Compile(
            "web-search MCP max_output_tokens must be positive, ordered, and at most 100000".into(),
        ));
    }
    if settings.allowed_domains.len() > 100 || settings.blocked_domains.len() > 100 {
        return Err(ConfigError::Compile(
            "web-search MCP domain lists support at most 100 entries each".into(),
        ));
    }
    let domain = Regex::new(
        r"(?i)^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)*[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$",
    )
    .expect("static MCP domain regex");
    let mut allowed = HashSet::new();
    for value in &mut settings.allowed_domains {
        let canonical = value.to_ascii_lowercase();
        if value.len() > 253 || !domain.is_match(value) || !allowed.insert(canonical.clone()) {
            return Err(ConfigError::Compile(
                "web-search MCP allowed_domains contains an invalid or duplicate domain".into(),
            ));
        }
        *value = canonical;
    }
    let mut blocked = HashSet::new();
    for value in &mut settings.blocked_domains {
        let canonical = value.to_ascii_lowercase();
        if value.len() > 253
            || !domain.is_match(value)
            || !blocked.insert(canonical.clone())
            || allowed.contains(&canonical)
        {
            return Err(ConfigError::Compile(
                "web-search MCP blocked_domains contains an invalid, duplicate, or allowed domain"
                    .into(),
            ));
        }
        *value = canonical;
    }
    Ok(())
}

fn compile_system_settings(
    record: SystemSettingsRecord,
) -> Result<SystemRuntimeSettings, ConfigError> {
    if record.setting_key != FORWARDING_SETTINGS_KEY {
        return Err(ConfigError::Compile(
            "required system settings are missing".into(),
        ));
    }
    let input = serde_json::from_value::<SystemSettingsInput>(record.value)
        .map_err(|_| ConfigError::Compile("invalid system settings".into()))?;
    compile_system_settings_input(&input)
}

/// Validates a decoded process-wide `system_settings` document.
pub fn compile_system_settings_input(
    input: &SystemSettingsInput,
) -> Result<SystemRuntimeSettings, ConfigError> {
    let upstream = &input.upstream;
    let request_retry = &input.request_retry;
    let passive_health = &input.passive_health;
    let automatic_disable = &input.automatic_disable;
    let scheduled_testing = &input.scheduled_testing;
    let session_affinity = compile_session_affinity_settings(&input.session_affinity)?;
    let websocket = &input.websocket;
    let mcp = compile_mcp_transport_settings(&input.mcp)?;
    if !valid_api_hosts(&input.api_hosts)
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
        || websocket.max_idle_connections > 4_096
        || websocket.idle_timeout_seconds == 0
        || websocket.idle_timeout_seconds > 3_600
        || websocket.max_connection_age_seconds < 60
        || websocket.max_connection_age_seconds > 3_600
        || websocket.idle_timeout_seconds >= websocket.max_connection_age_seconds
    {
        return Err(ConfigError::Compile("invalid system settings".into()));
    }
    let mut status_codes = automatic_disable.error_status_codes.clone();
    status_codes.sort_unstable();
    status_codes.dedup();
    let mut keywords = automatic_disable
        .error_message_keywords
        .iter()
        .map(|keyword| keyword.trim().to_owned())
        .collect::<Vec<_>>();
    keywords.sort_unstable_by_key(|keyword| keyword.to_lowercase());
    keywords.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
    let scheduled_mode = match scheduled_testing.mode.as_str() {
        "global" => ScheduledTestingMode::Global,
        "failure_only" => ScheduledTestingMode::FailureOnly,
        _ => unreachable!("validated scheduled testing mode"),
    };
    Ok(SystemRuntimeSettings::new_with_all_and_websocket(
        UpstreamTimeoutDefaults::new(
            std::time::Duration::from_secs(upstream.connect_timeout_seconds),
            std::time::Duration::from_secs(upstream.response_header_timeout_seconds),
            std::time::Duration::from_secs(upstream.stream_idle_timeout_seconds),
        )
        .with_images_response_header(std::time::Duration::from_secs(
            upstream.images_response_header_timeout_seconds,
        ))
        .with_standalone_web_search_response_header(std::time::Duration::from_secs(
            upstream.standalone_web_search_response_header_timeout_seconds,
        )),
        RequestRetrySettings::new(request_retry.enabled, request_retry.max_retries),
        PassiveHealthSettings::new(
            passive_health.connection_failure_threshold,
            std::time::Duration::from_secs(passive_health.cooldown_seconds),
        ),
        AutomaticDisableSettings::new(
            automatic_disable.enabled,
            status_codes.into(),
            keywords
                .into_iter()
                .map(Arc::<str>::from)
                .collect::<Vec<_>>()
                .into(),
        ),
        ScheduledTestingSettings::new(
            scheduled_mode,
            scheduled_testing.auto_recover,
            std::time::Duration::from_secs(scheduled_testing.interval_minutes.saturating_mul(60)),
            Arc::from(scheduled_testing.prompt.trim()),
        ),
        session_affinity,
        ResponsesWebSocketSettings::new(
            websocket.enabled,
            websocket.max_idle_connections,
            std::time::Duration::from_secs(websocket.idle_timeout_seconds),
            std::time::Duration::from_secs(websocket.max_connection_age_seconds),
        ),
    )
    .with_mcp(mcp))
}

fn compile_mcp_transport_settings(
    input: &SystemMcpSettingsInput,
) -> Result<McpTransportSettings, ConfigError> {
    if input.request_body_bytes == 0
        || input.image_request_body_bytes == 0
        || input.search_result_bytes == 0
        || input.image_result_bytes == 0
    {
        return Err(ConfigError::Compile(
            "mcp request and result limits must be greater than zero".into(),
        ));
    }
    if input.image_result_bytes > MAX_MCP_IMAGE_BYTES {
        return Err(ConfigError::Compile(
            "mcp image_result_bytes must not exceed 67108864".into(),
        ));
    }
    if input.image_request_body_bytes > MAX_MCP_IMAGE_BYTES {
        return Err(ConfigError::Compile(
            "mcp image_request_body_bytes must not exceed 67108864".into(),
        ));
    }
    if input.allowed_origins.len() > 64 {
        return Err(ConfigError::Compile(
            "mcp allowed_origins must contain at most 64 entries".into(),
        ));
    }
    if input.enabled && !cfg!(feature = "mcp-server") {
        return Err(ConfigError::Compile(
            "mcp enabled requires building with the mcp-server cargo feature".into(),
        ));
    }
    let parsed = input
        .public_base_url
        .as_deref()
        .map(|value| {
            if value.trim() != value || value.chars().count() > 2_048 {
                return Err(ConfigError::Compile(
                    "mcp public_base_url is invalid".into(),
                ));
            }
            parse_http_origin(value, "mcp public_base_url")
        })
        .transpose()?;
    if input.enabled && parsed.is_none() {
        return Err(ConfigError::Compile(
            "enabled mcp public_base_url is required".into(),
        ));
    }
    let allowed_origins = input
        .allowed_origins
        .iter()
        .map(|origin| {
            if origin.trim() != origin || origin.chars().count() > 2_048 {
                return Err(ConfigError::Compile("mcp allowed origin is invalid".into()));
            }
            canonical_http_origin(origin, "mcp allowed origin")
        })
        .collect::<Result<Vec<_>, _>>()?;
    unique(&allowed_origins, "mcp allowed origin")?;
    let mut allowed_hosts = Vec::new();
    let public_base_url = parsed.map(|parsed| {
        let host = parsed
            .host_str()
            .expect("validated MCP public URL has a host");
        let mut authority = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        if let Some(port) = parsed.port() {
            authority.push(':');
            authority.push_str(&port.to_string());
        }
        allowed_hosts.push(authority.clone());
        if parsed.port().is_none() {
            let default_port = match parsed.scheme() {
                "https" => 443,
                "http" => 80,
                _ => unreachable!("validated MCP URL scheme"),
            };
            allowed_hosts.push(format!("{authority}:{default_port}"));
        }
        Arc::<str>::from(parsed.origin().ascii_serialization())
    });
    Ok(McpTransportSettings::new(
        input.enabled,
        public_base_url,
        allowed_hosts.into(),
        allowed_origins.into(),
        input.allow_legacy_2025_11_25,
        input.request_body_bytes,
        input.image_request_body_bytes,
        input.search_result_bytes,
        input.image_result_bytes,
    ))
}

const MAX_SESSION_AFFINITY_ENTRIES: usize = 1_000_000;
const MAX_SESSION_AFFINITY_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_SESSION_AFFINITY_RULES: usize = 64;
const MAX_SESSION_AFFINITY_REGEXES: usize = 8;
const MAX_SESSION_AFFINITY_KEY_SOURCES: usize = 8;
const MAX_SESSION_AFFINITY_NAME_CHARS: usize = 64;
const MAX_SESSION_AFFINITY_PATTERN_CHARS: usize = 256;

fn compile_session_affinity_settings(
    input: &SystemSessionAffinitySettingsInput,
) -> Result<SessionAffinitySettings, ConfigError> {
    if input.max_entries == 0
        || input.max_entries > MAX_SESSION_AFFINITY_ENTRIES
        || input.default_ttl_seconds == 0
        || input.default_ttl_seconds > MAX_SESSION_AFFINITY_TTL_SECONDS
        || input.rules.len() > MAX_SESSION_AFFINITY_RULES
    {
        return Err(ConfigError::Compile(
            "session affinity settings are invalid".into(),
        ));
    }

    let mut names = HashSet::new();
    let mut compiled = Vec::new();
    for rule in &input.rules {
        let name = rule.name.trim();
        let normalized_name = name.to_lowercase();
        if name.is_empty()
            || name.chars().count() > MAX_SESSION_AFFINITY_NAME_CHARS
            || !names.insert(normalized_name)
            || rule.api_formats.is_empty()
            || rule.model_regex.len() > MAX_SESSION_AFFINITY_REGEXES
            || rule.key_sources.is_empty()
            || rule.key_sources.len() > MAX_SESSION_AFFINITY_KEY_SOURCES
        {
            return Err(ConfigError::Compile(
                "session affinity rule is invalid".into(),
            ));
        }

        unique(&rule.api_formats, "session affinity api_formats")?;
        unique(&rule.model_regex, "session affinity model_regex")?;
        let api_formats = rule
            .api_formats
            .iter()
            .map(|value| parse_format(value))
            .collect::<Result<Vec<_>, _>>()?;
        if api_formats.contains(&ApiFormat::OpenAiImages) {
            return Err(ConfigError::Compile(
                "Images requests do not support session affinity".into(),
            ));
        }
        let model_regex = rule
            .model_regex
            .iter()
            .map(|pattern| compile_session_affinity_regex(pattern))
            .collect::<Result<Vec<_>, _>>()?;
        let value_regex = rule
            .value_regex
            .as_deref()
            .map(compile_session_affinity_regex)
            .transpose()?;
        let key_sources = rule
            .key_sources
            .iter()
            .map(compile_session_affinity_key_source)
            .collect::<Result<Vec<_>, _>>()?;
        let ttl_seconds = rule.ttl_seconds.unwrap_or(input.default_ttl_seconds);
        if ttl_seconds == 0 || ttl_seconds > MAX_SESSION_AFFINITY_TTL_SECONDS {
            return Err(ConfigError::Compile(
                "session affinity rule TTL is invalid".into(),
            ));
        }
        if !rule.enabled {
            continue;
        }

        compiled.push(SessionAffinityRule::new(
            Arc::from(name),
            session_affinity_rule_fingerprint(rule, ttl_seconds),
            api_formats.into(),
            model_regex.into(),
            key_sources.into(),
            value_regex,
            std::time::Duration::from_secs(ttl_seconds),
        ));
    }

    Ok(SessionAffinitySettings::new(
        input.enabled,
        input.max_entries,
        std::time::Duration::from_secs(input.default_ttl_seconds),
        compiled.into(),
    ))
}

fn compile_session_affinity_regex(pattern: &str) -> Result<Regex, ConfigError> {
    if pattern.is_empty() || pattern.chars().count() > MAX_SESSION_AFFINITY_PATTERN_CHARS {
        return Err(ConfigError::Compile(
            "session affinity regex is invalid".into(),
        ));
    }
    Regex::new(pattern)
        .map_err(|_| ConfigError::Compile("session affinity regex is invalid".into()))
}

fn compile_session_affinity_key_source(
    source: &SystemSessionAffinityKeySourceInput,
) -> Result<SessionAffinityKeySource, ConfigError> {
    match source {
        SystemSessionAffinityKeySourceInput::RequestHeader { name } => {
            let name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| {
                ConfigError::Compile("session affinity request header is invalid".into())
            })?;
            if matches!(
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
            ) {
                return Err(ConfigError::Compile(
                    "session affinity request header is unsafe".into(),
                ));
            }
            if !client_header_allowed(&name) {
                return Err(ConfigError::Compile(
                    "session affinity request header is not allowed by the client request policy"
                        .into(),
                ));
            }
            Ok(SessionAffinityKeySource::RequestHeader(name))
        }
        SystemSessionAffinityKeySourceInput::JsonPointer { pointer } => {
            let pointer = pointer.trim();
            if pointer.chars().count() > MAX_SESSION_AFFINITY_PATTERN_CHARS
                || !valid_json_pointer(pointer)
            {
                return Err(ConfigError::Compile(
                    "session affinity JSON pointer is invalid".into(),
                ));
            }
            Ok(SessionAffinityKeySource::JsonPointer(Arc::from(pointer)))
        }
    }
}

fn valid_json_pointer(pointer: &str) -> bool {
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

fn session_affinity_rule_fingerprint(
    rule: &SystemSessionAffinityRuleInput,
    ttl_seconds: u64,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway:session-affinity:v1\0");
    hasher.update(rule.name.trim().as_bytes());
    hasher.update([0]);
    for format in &rule.api_formats {
        hasher.update(format.as_bytes());
        hasher.update([0]);
    }
    for pattern in &rule.model_regex {
        hasher.update(pattern.as_bytes());
        hasher.update([0]);
    }
    hasher.update(
        serde_json::to_vec(&rule.key_sources).expect("session affinity key sources serialize"),
    );
    hasher.update([0]);
    if let Some(pattern) = &rule.value_regex {
        hasher.update(pattern.as_bytes());
    }
    hasher.update([0]);
    hasher.update(ttl_seconds.to_le_bytes());
    hasher.finalize().into()
}

fn compile_proxies(
    records: Vec<ProxyRecord>,
) -> Result<HashMap<Uuid, Arc<CompiledProxy>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("proxy id"));
        }
        let id = record.id;
        let enabled = record.enabled;
        let proxy = compile_proxy_record(record)?;
        if enabled {
            result.insert(id, proxy);
        }
    }
    Ok(result)
}

pub(crate) fn compile_proxy_test_target(
    record: ProxyRecord,
) -> Result<Arc<CompiledProxy>, ConfigError> {
    compile_proxy_record(record)
}

fn compile_proxy_record(record: ProxyRecord) -> Result<Arc<CompiledProxy>, ConfigError> {
    require("proxy name", &record.name)?;
    let url = Url::parse(&record.proxy_url)
        .map_err(|_| ConfigError::Compile("proxy has an invalid URL".into()))?;
    if !matches!(
        url.scheme(),
        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
    ) || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConfigError::Compile(
            "proxy URL must use http, https, socks4, socks4a, socks5, or socks5h with a root path and without embedded credentials, query, or fragment".into(),
        ));
    }
    let no_proxy_hosts = record
        .no_proxy_hosts
        .iter()
        .map(|host| NoProxyHost::parse(host).map_err(|_| invalid_no_proxy_host()))
        .collect::<Result<Vec<_>, ConfigError>>()?;
    unique(&no_proxy_hosts, "proxy no_proxy_hosts")?;
    if let Some(username) = &record.username {
        require("proxy username", username)?;
    }
    if let Some(password) = &record.password {
        require("proxy password", password)?;
    }
    validate_proxy_credentials(
        url.scheme(),
        record.username.as_deref(),
        record.password.as_deref(),
    )?;
    Ok(Arc::new(CompiledProxy::new(
        record.id,
        Arc::from(record.name),
        url,
        record.username.map(Arc::from),
        record.password.map(Arc::from),
        no_proxy_hosts.into(),
    )))
}

fn validate_proxy_credentials(
    scheme: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<(), ConfigError> {
    match scheme {
        "socks4" | "socks4a" if username.is_some() || password.is_some() => Err(
            ConfigError::Compile("SOCKS4 proxy credentials are not supported".into()),
        ),
        "socks5" | "socks5h" if username.is_some() != password.is_some() => {
            Err(ConfigError::Compile(
                "SOCKS5 proxy credentials must include both username and password".into(),
            ))
        }
        "socks5" | "socks5h" => {
            for credential in [username, password].into_iter().flatten() {
                if !(1..=255).contains(&credential.len()) {
                    return Err(ConfigError::Compile(
                        "SOCKS5 proxy credentials have an invalid length".into(),
                    ));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn compile_templates(
    records: Vec<ConfigTemplateRecord>,
) -> Result<HashMap<Uuid, Arc<CompiledConfigTemplate>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("config template id"));
        }
        require("config template name", &record.name)?;
        let declared = declared_api_format(&record.document)
            .map_err(transform_error("config template document"))?;
        let chat = if declared.is_none() || declared == Some(ApiFormat::OpenAiChatCompletions) {
            compile_document(&record.document, ApiFormat::OpenAiChatCompletions)
                .map_err(transform_error("config template document"))?
        } else {
            TransformPlan::noop(ApiFormat::OpenAiChatCompletions)
        };
        let responses = if declared.is_none() || declared == Some(ApiFormat::OpenAiResponses) {
            compile_document(&record.document, ApiFormat::OpenAiResponses)
                .map_err(transform_error("config template document"))?
        } else {
            TransformPlan::noop(ApiFormat::OpenAiResponses)
        };
        let images = if declared.is_none() || declared == Some(ApiFormat::OpenAiImages) {
            compile_document(&record.document, ApiFormat::OpenAiImages)
                .map_err(transform_error("config template document"))?
        } else {
            TransformPlan::noop(ApiFormat::OpenAiImages)
        };
        if record.enabled {
            result.insert(
                record.id,
                Arc::new(CompiledConfigTemplate::new(
                    record.id,
                    Arc::from(record.name),
                    record.description.map(Arc::from),
                    declared,
                    chat,
                    responses,
                    images,
                )),
            );
        }
    }
    Ok(result)
}

fn compile_channel_document(
    channel: &ChannelRecord,
    format: ApiFormat,
) -> Result<TransformPlan, ConfigError> {
    compile_document(&channel.override_document, format)
        .map_err(transform_error("channel override document"))
}

/// Compiles the outbound portion of an unsaved channel draft for explicit
/// administrator operations such as `GET /v1/models` discovery.
pub(crate) fn compile_channel_discovery_target(
    channel: &ChannelRecord,
    snapshot: &CompiledRuntimeConfig,
) -> Result<CompiledChannel, ConfigError> {
    let api_format = parse_format(&channel.api_format)?;
    let proxy = channel
        .proxy_id
        .map(|id| {
            snapshot.proxy(id).ok_or_else(|| {
                ConfigError::Compile("channel references a missing or disabled proxy".into())
            })
        })
        .transpose()?;
    let template = channel
        .config_template_id
        .map(|id| {
            snapshot.template(id).ok_or_else(|| {
                ConfigError::Compile("channel references a missing or disabled template".into())
            })
        })
        .transpose()?;
    if template
        .as_ref()
        .and_then(|template| template.api_format())
        .is_some_and(|template_format| template_format != api_format)
    {
        return Err(ConfigError::Compile(
            "channel references a cross-format template".into(),
        ));
    }
    let channel_override = compile_channel_document(channel, api_format)?;
    let defaults = template.as_ref().map_or_else(
        || TransformPlan::noop(api_format),
        |template| template.transform_plan(api_format).clone(),
    );
    let effective_transforms = TransformPlan::compose(&defaults, &channel_override)
        .map_err(transform_error("channel effective transform plan"))?;
    let upstream_policy = CompiledChannelUpstreamPolicy::new_with_default_connect_timeout(
        proxy,
        template,
        channel_override,
        effective_transforms,
        compile_timeouts(channel)?,
        snapshot.system_settings().upstream_timeouts().connect(),
    );
    Ok(CompiledChannel::new_with_policy(
        channel.id,
        channel.channel_group_id,
        api_format,
        parse_url(channel.id, &channel.base_url)?,
        channel.weight,
        compile_auth(channel)?,
        HashSet::new(),
        upstream_policy,
    ))
}

fn compile_timeouts(channel: &ChannelRecord) -> Result<ChannelTimeoutPolicy, ConfigError> {
    Ok(ChannelTimeoutPolicy::new(
        positive_timeout(channel.connect_timeout_ms, "connect_timeout_ms")?,
        positive_timeout(
            channel.response_header_timeout_ms,
            "response_header_timeout_ms",
        )?,
        positive_timeout(channel.stream_idle_timeout_ms, "stream_idle_timeout_ms")?,
    ))
}

fn positive_timeout(
    value: Option<i32>,
    name: &str,
) -> Result<Option<std::time::Duration>, ConfigError> {
    value
        .map(|value| {
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .map(std::time::Duration::from_millis)
                .ok_or_else(|| {
                    ConfigError::Compile(format!("channel {name} must be positive when configured"))
                })
        })
        .transpose()
}

fn transform_error(context: &'static str) -> impl FnOnce(TransformCompileError) -> ConfigError {
    move |error| ConfigError::Compile(format!("{context} is invalid: {error}"))
}

fn compile_keys(
    records: Vec<ApiKeyRecord>,
    all_groups: &HashMap<Uuid, ChannelGroupRecord>,
    all_channels: &HashMap<Uuid, ChannelRecord>,
    channels_by_group: &HashMap<Uuid, Vec<Uuid>>,
    channel_slots: &HashMap<Uuid, usize>,
    model_rules: &HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    routes_by_channel_slot: &[Vec<usize>],
) -> Result<HashMap<ApiKeyHash, Arc<CompiledApiKey>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    let mut authorization_profiles = HashMap::<Vec<u64>, Arc<AuthorizationProfile>>::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("API key id"));
        }
        validate_key(&record, all_groups, all_channels)?;
        let usable = record.status == "active" && record.user_status == "active";
        if !usable {
            continue;
        }
        let formats = record
            .allowed_api_formats
            .iter()
            .map(|value| parse_format(value))
            .collect::<Result<HashSet<_>, _>>()?;
        let permissions = record
            .permissions
            .iter()
            .map(|value| parse_permission(value))
            .collect::<Result<HashSet<_>, _>>()?;
        let (allowed_channel_slot_words, allowed_channel_ids) =
            compile_allowed_channel_slots(&record, channels_by_group, channel_slots);
        let authorization = authorization_profiles
            .entry(allowed_channel_slot_words.clone())
            .or_insert_with(|| {
                let accessible_route_slots = compile_accessible_route_slots(
                    model_rules,
                    routes_by_channel_slot,
                    &allowed_channel_ids,
                    &allowed_channel_slot_words,
                    channel_slots,
                );
                Arc::new(AuthorizationProfile::new(
                    allowed_channel_ids,
                    allowed_channel_slot_words.into(),
                    accessible_route_slots.into(),
                ))
            })
            .clone();
        let secret = Zeroizing::new(record.secret_value);
        let key = Arc::new(CompiledApiKey::new_with_authorization_profile(
            record.id,
            record.user_id,
            record.user_websocket_enabled,
            formats,
            permissions,
            authorization,
            record.expires_at,
            positive_policy(record.requests_per_minute, "requests_per_minute")?,
            positive_policy(record.max_concurrent_requests, "max_concurrent_requests")?,
            record.quota_limit_amount,
            record.quota_used_amount,
        ));
        if result
            .insert(ApiKeyHash::from_secret(secret.as_str()), key)
            .is_some()
        {
            return Err(ConfigError::Compile(
                "duplicate active API key secret".into(),
            ));
        }
    }
    Ok(result)
}

fn compile_allowed_channel_slots(
    record: &ApiKeyRecord,
    channels_by_group: &HashMap<Uuid, Vec<Uuid>>,
    channel_slots: &HashMap<Uuid, usize>,
) -> (Vec<u64>, HashSet<Uuid>) {
    let mut words = vec![0_u64; channel_slots.len().div_ceil(u64::BITS as usize)];
    let mut allowed_channel_ids = HashSet::new();
    let mut allow = |channel_id: &Uuid| {
        let slot = channel_slots[channel_id];
        words[slot / u64::BITS as usize] |= 1_u64 << (slot % u64::BITS as usize);
        allowed_channel_ids.insert(*channel_id);
    };
    for group_id in &record.allowed_group_ids {
        if let Some(channel_ids) = channels_by_group.get(group_id) {
            for channel_id in channel_ids {
                allow(channel_id);
            }
        }
    }
    for channel_id in &record.allowed_channel_ids {
        allow(channel_id);
    }
    (words, allowed_channel_ids)
}

fn compile_accessible_route_slots(
    model_rules: &HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    routes_by_channel_slot: &[Vec<usize>],
    allowed_channel_ids: &HashSet<Uuid>,
    allowed_channel_slots: &[u64],
    channel_slots: &HashMap<Uuid, usize>,
) -> Vec<u64> {
    let mut words = vec![0_u64; model_rules.len().div_ceil(u64::BITS as usize)];
    for channel_id in allowed_channel_ids {
        for route_slot in &routes_by_channel_slot[channel_slots[channel_id]] {
            words[route_slot / u64::BITS as usize] |= 1_u64 << (route_slot % u64::BITS as usize);
        }
    }
    debug_assert!(model_rules.values().all(|rule| {
        let accessible = words
            .get(rule.route_slot() / u64::BITS as usize)
            .is_some_and(|bits| bits & (1_u64 << (rule.route_slot() % u64::BITS as usize)) != 0);
        accessible == rule.configured_candidates_intersect(allowed_channel_slots)
    }));
    words
}

fn index_models(records: Vec<ModelRecord>) -> Result<HashMap<String, ModelRecord>, ConfigError> {
    let mut ids = HashSet::new();
    let mut by_source = HashMap::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("model id"));
        }
        if by_source
            .insert(record.source_model_id.clone(), record)
            .is_some()
        {
            return Err(dup("model source id"));
        }
    }
    Ok(by_source)
}

fn compile_scheduled_test_models(
    channels: &HashMap<Uuid, ChannelRecord>,
    models_by_source: &HashMap<String, ModelRecord>,
) -> Result<HashMap<Arc<str>, Arc<CompiledScheduledTestModel>>, ConfigError> {
    let mut result = HashMap::new();
    for channel in channels.values() {
        let Some(test_model) = channel.test_model.as_deref() else {
            continue;
        };
        let model = models_by_source.get(test_model).ok_or_else(|| {
            ConfigError::Compile(
                "channel test model must reference a configured priced model".into(),
            )
        })?;
        if !result.contains_key(test_model) {
            result.insert(
                Arc::from(test_model),
                Arc::new(compile_scheduled_test_model(model)?),
            );
        }
        let scheduled_test_model = result
            .get(test_model)
            .expect("scheduled test model was just inserted or already present");
        validate_effective_scheduled_test_prices(scheduled_test_model, channel.billing_multiplier)?;
    }
    Ok(result)
}

fn compile_scheduled_test_model(
    record: &ModelRecord,
) -> Result<CompiledScheduledTestModel, ConfigError> {
    if record.currency != "USD"
        || record.price_unit_tokens <= 0
        || [
            record.input_unit_price,
            record.cached_input_unit_price,
            record.cache_write_unit_price,
            record.output_unit_price,
        ]
        .into_iter()
        .any(|price| price.is_sign_negative())
    {
        return Err(ConfigError::Compile(
            "scheduled test model has invalid price metadata".into(),
        ));
    }
    let advanced_billing =
        serde_json::from_value::<AdvancedBilling>(record.advanced_billing.clone())
            .map_err(|_| ConfigError::Compile("invalid advanced billing configuration".into()))?;
    let advanced_billing = crate::domain::CompiledAdvancedBilling::compile(advanced_billing)
        .map_err(|_| ConfigError::Compile("invalid advanced billing configuration".into()))?;
    Ok(CompiledScheduledTestModel::new(
        record.id,
        ModelPriceSnapshot::new(
            Arc::from(record.currency.as_str()),
            record.price_unit_tokens,
            record.price_effective_at,
            record.input_unit_price,
            record.cached_input_unit_price,
            record.cache_write_unit_price,
            record.output_unit_price,
        ),
        advanced_billing,
    ))
}

fn validate_effective_scheduled_test_prices(
    model: &CompiledScheduledTestModel,
    billing_multiplier: rust_decimal::Decimal,
) -> Result<(), ConfigError> {
    let max_persisted_unit_price =
        rust_decimal::Decimal::from_i128_with_scale(999_999_999_999_999_999_999_999, 12);
    let request_multiplier = model.advanced_billing().maximum_request_multiplier();
    let snapshot = model.price_snapshot();
    for price in model.advanced_billing().price_candidates(
        snapshot.input_unit_price(),
        snapshot.cached_input_unit_price(),
        snapshot.cache_write_unit_price(),
        snapshot.output_unit_price(),
    ) {
        let Some(effective) = price
            .checked_mul(billing_multiplier)
            .and_then(|price| price.checked_mul(request_multiplier))
        else {
            return Err(ConfigError::Compile(
                "advanced billing multiplier overflows scheduled test price".into(),
            ));
        };
        if effective.round_dp(12) > max_persisted_unit_price {
            return Err(ConfigError::Compile(
                "effective scheduled test price exceeds request-log precision".into(),
            ));
        }
    }
    Ok(())
}

struct CompiledRules {
    model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    routes_by_channel_slot: Vec<Vec<usize>>,
}

fn compile_rules(
    records: Vec<ModelRuleRecord>,
    all_groups: &HashMap<Uuid, ChannelGroupRecord>,
    all_channels: &HashMap<Uuid, ChannelRecord>,
    channels_by_group_model: &HashMap<Uuid, HashMap<String, Vec<Uuid>>>,
    channel_slots: &HashMap<Uuid, usize>,
    groups: &HashMap<Uuid, Arc<CompiledChannelGroup>>,
    channels: &HashMap<Uuid, Arc<CompiledChannel>>,
) -> Result<CompiledRules, ConfigError> {
    let mut result = HashMap::new();
    let mut routes_by_channel_slot = vec![Vec::new(); channel_slots.len()];
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("model rule id"));
        }
        validate_rule(&record)?;
        validate_rule_references(&record, all_groups, all_channels)?;
        if !record.enabled {
            continue;
        }
        if !record.upstream_model_enabled {
            return Err(ConfigError::Compile(
                "enabled model rule references a disabled upstream model".into(),
            ));
        }
        let format = parse_format(&record.api_format)?;
        let mut candidates = HashSet::new();
        let mut unavailable_candidates = HashMap::<Uuid, Uuid>::new();
        let mut selected_candidates = HashSet::new();
        let mut tier_strategies = HashMap::<i32, SelectionStrategy>::new();
        for group_id in &record.channel_group_ids {
            let group = all_groups.get(group_id).ok_or_else(|| {
                ConfigError::Compile("enabled model rule references a missing channel group".into())
            })?;
            if parse_format(&group.api_format)? != format {
                return Err(ConfigError::Compile(
                    "enabled model rule references a cross-format channel group".into(),
                ));
            }
            for channel_id in channels_by_group_model
                .get(group_id)
                .and_then(|models| models.get(&record.upstream_model))
                .into_iter()
                .flatten()
            {
                let channel = &all_channels[channel_id];
                validate_effective_channel_prices(&record, channel.billing_multiplier)?;
                validate_route_tier_strategy(&mut tier_strategies, group)?;
                if !selected_candidates.insert(*channel_id) {
                    continue;
                }
                if group.enabled && channel.enabled && !channel.auto_disabled {
                    candidates.insert(*channel_id);
                } else {
                    unavailable_candidates.insert(*channel_id, *group_id);
                }
            }
        }
        for channel_id in &record.channel_ids {
            let channel = all_channels.get(channel_id).ok_or_else(|| {
                ConfigError::Compile("enabled model rule references a missing channel".into())
            })?;
            if parse_format(&channel.api_format)? != format {
                return Err(ConfigError::Compile(
                    "enabled model rule references a cross-format channel".into(),
                ));
            }
            if !channel
                .available_models
                .iter()
                .any(|model| model == &record.upstream_model)
            {
                return Err(ConfigError::Compile(
                    "direct channel candidate does not support the model rule upstream model"
                        .into(),
                ));
            }
            let group = all_groups.get(&channel.channel_group_id).ok_or_else(|| {
                ConfigError::Compile("direct channel candidate references a missing group".into())
            })?;
            validate_route_tier_strategy(&mut tier_strategies, group)?;
            validate_effective_channel_prices(&record, channel.billing_multiplier)?;
            if !selected_candidates.insert(*channel_id) {
                continue;
            }
            if group.enabled && channel.enabled && !channel.auto_disabled {
                candidates.insert(*channel_id);
            } else {
                unavailable_candidates.insert(*channel_id, channel.channel_group_id);
            }
        }
        if selected_candidates.is_empty() {
            return Err(ConfigError::Compile(
                "each enabled model rule must have at least one distinct model-capable candidate channel".into(),
            ));
        }
        let mut tier_channels: HashMap<i32, Vec<CompiledCandidate>> = HashMap::new();
        for candidate in candidates {
            let channel = &channels[&candidate];
            let group = groups.get(&channel.group_id()).ok_or_else(|| {
                ConfigError::Compile("eligible channel has no enabled group".into())
            })?;
            tier_channels
                .entry(group.priority())
                .or_default()
                .push(CompiledCandidate::new(
                    channel_slots[&candidate],
                    Arc::clone(channel),
                ));
        }
        let mut priorities = tier_channels.keys().copied().collect::<Vec<_>>();
        priorities.sort_unstable();
        let mut tiers = Vec::with_capacity(priorities.len());
        for priority in priorities {
            let mut tier_candidates = tier_channels
                .remove(&priority)
                .expect("priority was collected");
            tier_candidates.sort_unstable_by_key(|candidate| candidate.channel().id());
            let strategy = tier_strategies[&priority];
            let mut aggregate_weight = 0_i64;
            for candidate in &tier_candidates {
                aggregate_weight = aggregate_weight
                    .checked_add(i64::from(candidate.weight()))
                    .ok_or_else(|| {
                        ConfigError::Compile(
                            "route tier aggregate channel weight overflowed".into(),
                        )
                    })?;
            }
            if aggregate_weight <= 0 {
                return Err(ConfigError::Compile(
                    "route tier aggregate channel weight must be positive".into(),
                ));
            }
            tiers.push(CompiledRouteTier::new(
                priority,
                strategy,
                Arc::from(tier_candidates),
            ));
        }
        let mut unavailable_candidates = unavailable_candidates
            .into_iter()
            .map(|(channel_id, group_id)| {
                crate::domain::CompiledUnavailableRouteCandidate::new(channel_id, group_id)
            })
            .collect::<Vec<_>>();
        unavailable_candidates
            .sort_unstable_by_key(crate::domain::CompiledUnavailableRouteCandidate::channel_id);
        let mut configured_candidates =
            vec![0_u64; channel_slots.len().div_ceil(u64::BITS as usize)];
        for channel_id in &selected_candidates {
            let slot = channel_slots[channel_id];
            configured_candidates[slot / u64::BITS as usize] |=
                1_u64 << (slot % u64::BITS as usize);
        }
        let key = ModelRouteKey::new(format, Arc::<str>::from(record.client_model.as_str()));
        let route_slot = result.len();
        for channel_id in &selected_candidates {
            routes_by_channel_slot[channel_slots[channel_id]].push(route_slot);
        }
        let price_snapshot = compile_model_price_snapshot(&record)?;
        let advanced_billing = compile_advanced_billing(&record)?;
        let rule = Arc::new(CompiledModelRule::new_with_unavailable_candidates(
            route_slot,
            record.id,
            record.upstream_model_id,
            Arc::from(record.client_model),
            format,
            Arc::from(record.upstream_model),
            price_snapshot,
            advanced_billing,
            Arc::from(tiers),
            Arc::from(unavailable_candidates),
            Arc::from(configured_candidates),
        ));
        if result.insert(key, rule).is_some() {
            return Err(ConfigError::Compile(
                "duplicate enabled model rule for the same client model and API format".into(),
            ));
        }
    }
    Ok(CompiledRules {
        model_rules: result,
        routes_by_channel_slot,
    })
}

fn validate_route_tier_strategy(
    strategies: &mut HashMap<i32, SelectionStrategy>,
    group: &ChannelGroupRecord,
) -> Result<(), ConfigError> {
    let strategy = parse_strategy(&group.selection_strategy)?;
    if strategies
        .insert(group.priority, strategy)
        .is_some_and(|existing| existing != strategy)
    {
        return Err(ConfigError::Compile(
            "all channel groups in every route priority tier must use the same selection strategy"
                .into(),
        ));
    }
    Ok(())
}

fn validate_effective_channel_prices(
    record: &ModelRuleRecord,
    billing_multiplier: rust_decimal::Decimal,
) -> Result<(), ConfigError> {
    let max_persisted_unit_price =
        rust_decimal::Decimal::from_i128_with_scale(999_999_999_999_999_999_999_999, 12);
    let advanced_billing = compile_advanced_billing(record)?;
    let request_multiplier = advanced_billing.maximum_request_multiplier();
    for price in advanced_billing.price_candidates(
        record.input_unit_price,
        record.cached_input_unit_price,
        record.cache_write_unit_price,
        record.output_unit_price,
    ) {
        let Some(effective) = price
            .checked_mul(billing_multiplier)
            .and_then(|price| price.checked_mul(request_multiplier))
        else {
            return Err(ConfigError::Compile(
                "advanced billing multiplier overflows the effective model price".into(),
            ));
        };
        if effective.round_dp(12) > max_persisted_unit_price {
            return Err(ConfigError::Compile(
                "effective channel model price exceeds request-log precision".into(),
            ));
        }
    }
    Ok(())
}
fn compile_advanced_billing(
    record: &ModelRuleRecord,
) -> Result<crate::domain::CompiledAdvancedBilling, ConfigError> {
    let advanced_billing =
        serde_json::from_value::<AdvancedBilling>(record.advanced_billing.clone())
            .map_err(|_| ConfigError::Compile("invalid advanced billing configuration".into()))?;
    crate::domain::CompiledAdvancedBilling::compile(advanced_billing)
        .map_err(|_| ConfigError::Compile("invalid advanced billing configuration".into()))
}
fn compile_model_price_snapshot(
    record: &ModelRuleRecord,
) -> Result<ModelPriceSnapshot, ConfigError> {
    if record.upstream_model_currency != "USD"
        || record.price_unit_tokens <= 0
        || [
            record.input_unit_price,
            record.cached_input_unit_price,
            record.cache_write_unit_price,
            record.output_unit_price,
        ]
        .into_iter()
        .any(|price| price.is_sign_negative())
    {
        return Err(ConfigError::Compile(
            "model rule references invalid upstream-model price metadata".into(),
        ));
    }
    Ok(ModelPriceSnapshot::new(
        Arc::from(record.upstream_model_currency.as_str()),
        record.price_unit_tokens,
        record.price_effective_at,
        record.input_unit_price,
        record.cached_input_unit_price,
        record.cache_write_unit_price,
        record.output_unit_price,
    ))
}
fn validate_rule_references(
    record: &ModelRuleRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
    channels: &HashMap<Uuid, ChannelRecord>,
) -> Result<(), ConfigError> {
    let format = parse_format(&record.api_format)?;
    for group_id in &record.channel_group_ids {
        let group = groups.get(group_id).ok_or_else(|| {
            ConfigError::Compile("model rule references a missing channel group".into())
        })?;
        if parse_format(&group.api_format)? != format {
            return Err(ConfigError::Compile(
                "model rule references a cross-format channel group".into(),
            ));
        }
    }
    for channel_id in &record.channel_ids {
        let channel = channels.get(channel_id).ok_or_else(|| {
            ConfigError::Compile("model rule references a missing channel".into())
        })?;
        if parse_format(&channel.api_format)? != format {
            return Err(ConfigError::Compile(
                "model rule references a cross-format channel".into(),
            ));
        }
    }
    Ok(())
}

fn validate_group(record: &ChannelGroupRecord) -> Result<(), ConfigError> {
    require("channel group name", &record.name)?;
    let api_format = parse_format(&record.api_format)?;
    let connector_kind = parse_connector_kind(&record.connector_kind)?;
    if connector_kind == ConnectorKind::CodexOauth
        && !matches!(
            api_format,
            ApiFormat::OpenAiResponses | ApiFormat::OpenAiImages
        )
    {
        return Err(ConfigError::Compile(
            "Codex OAuth channel groups must use Responses or Images".into(),
        ));
    }
    if record.priority < 0 || SelectionStrategy::parse(&record.selection_strategy).is_none() {
        return Err(ConfigError::Compile(
            "invalid channel group selection metadata".into(),
        ));
    }
    Ok(())
}
fn parse_connector_kind(value: &str) -> Result<ConnectorKind, ConfigError> {
    ConnectorKind::parse(value)
        .ok_or_else(|| ConfigError::Compile("unsupported upstream connector kind".into()))
}
fn parse_strategy(value: &str) -> Result<SelectionStrategy, ConfigError> {
    SelectionStrategy::parse(value)
        .ok_or_else(|| ConfigError::Compile("unsupported channel group selection strategy".into()))
}
fn validate_channel(
    record: &ChannelRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
) -> Result<(), ConfigError> {
    require("channel name", &record.name)?;
    let format = parse_format(&record.api_format)?;
    if record.supports_websocket && format != ApiFormat::OpenAiResponses {
        return Err(ConfigError::Compile(
            "only Responses channels can support WebSocket forwarding".into(),
        ));
    }
    if record.supports_standalone_web_search && format != ApiFormat::OpenAiResponses {
        return Err(ConfigError::Compile(
            "only Responses channels can support standalone web search".into(),
        ));
    }
    compile_document(&record.override_document, format)
        .map_err(transform_error("channel override document"))?;
    compile_timeouts(record)?;
    if record.weight <= 0 {
        return Err(ConfigError::Compile(
            "channel weight must be positive".into(),
        ));
    }
    if record.billing_multiplier.is_sign_negative() {
        return Err(ConfigError::Compile(
            "channel billing multiplier must be non-negative".into(),
        ));
    }
    unique(&record.available_models, "channel available_models")?;
    for model in &record.available_models {
        require("channel available model", model)?;
    }
    if let Some(test_model) = &record.test_model {
        if format == ApiFormat::OpenAiImages {
            return Err(ConfigError::Compile(
                "Images channels do not support scheduled test models".into(),
            ));
        }
        require("channel test model", test_model)?;
        if !record
            .available_models
            .iter()
            .any(|model| model == test_model)
        {
            return Err(ConfigError::Compile(
                "channel test model must be one of its available models".into(),
            ));
        }
    }
    if !matches!(
        record.upstream_auth_kind.as_str(),
        "none" | "bearer" | "header"
    ) {
        return Err(ConfigError::Compile(
            "unsupported upstream auth kind".into(),
        ));
    }
    let group = groups
        .get(&record.channel_group_id)
        .ok_or_else(|| ConfigError::Compile("channel references a missing group".into()))?;
    if parse_format(&group.api_format)? != format {
        return Err(ConfigError::Compile(
            "channel and group use different API formats".into(),
        ));
    }
    let connector_kind = parse_connector_kind(&group.connector_kind)?;
    let codex_protocol_valid = match format {
        ApiFormat::OpenAiResponses => record.supports_websocket,
        ApiFormat::OpenAiImages => {
            !record.supports_websocket && !record.supports_standalone_web_search
        }
        ApiFormat::OpenAiChatCompletions => false,
    };
    if connector_kind == ConnectorKind::CodexOauth
        && (!codex_protocol_valid
            || record.upstream_auth_kind != "none"
            || record.upstream_auth_header_name.is_some()
            || record.upstream_api_key.is_some()
            || record.test_model.is_some())
    {
        return Err(ConfigError::Compile(
            "invalid Codex OAuth managed channel configuration".into(),
        ));
    }
    if record.enabled {
        parse_url(record.id, &record.base_url)?;
        compile_auth(record)?;
    }
    Ok(())
}
fn validate_channel_resources(
    record: &ChannelRecord,
    proxies: &HashMap<Uuid, Arc<CompiledProxy>>,
    templates: &HashMap<Uuid, Arc<CompiledConfigTemplate>>,
) -> Result<(), ConfigError> {
    let format = parse_format(&record.api_format)?;
    if record.proxy_id.is_some_and(|id| !proxies.contains_key(&id)) {
        return Err(ConfigError::Compile(
            "channel references a missing or disabled proxy".into(),
        ));
    }
    if let Some(template_id) = record.config_template_id {
        let template = templates.get(&template_id).ok_or_else(|| {
            ConfigError::Compile("channel references a missing or disabled template".into())
        })?;
        if template
            .api_format()
            .is_some_and(|template_format| template_format != format)
        {
            return Err(ConfigError::Compile(
                "channel references a cross-format template".into(),
            ));
        }
    }
    Ok(())
}
fn validate_key(
    record: &ApiKeyRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
    channels: &HashMap<Uuid, ChannelRecord>,
) -> Result<(), ConfigError> {
    require("API key secret", &record.secret_value)?;
    if !matches!(
        record.status.as_str(),
        "active" | "disabled" | "revoked" | "expired"
    ) || !matches!(
        record.user_status.as_str(),
        "active" | "suspended" | "disabled"
    ) {
        return Err(ConfigError::Compile(
            "invalid API key or user status".into(),
        ));
    }
    if record.allowed_api_formats.is_empty() || record.permissions.is_empty() {
        return Err(ConfigError::Compile(
            "API key must allow formats and grant permissions".into(),
        ));
    }
    unique(&record.allowed_api_formats, "API key allowed_api_formats")?;
    unique(&record.permissions, "API key permissions")?;
    let formats = record
        .allowed_api_formats
        .iter()
        .map(|value| parse_format(value))
        .collect::<Result<HashSet<_>, _>>()?;
    for permission in &record.permissions {
        parse_permission(permission)?;
    }
    unique(&record.allowed_group_ids, "API key allowed_group_ids")?;
    unique(&record.allowed_channel_ids, "API key allowed_channel_ids")?;
    for id in &record.allowed_group_ids {
        let group = groups
            .get(id)
            .ok_or_else(|| ConfigError::Compile("API key references a missing group".into()))?;
        if !formats.contains(&parse_format(&group.api_format)?) {
            return Err(ConfigError::Compile(
                "API key group access references a disallowed format".into(),
            ));
        }
    }
    for id in &record.allowed_channel_ids {
        let channel = channels
            .get(id)
            .ok_or_else(|| ConfigError::Compile("API key references a missing channel".into()))?;
        if !formats.contains(&parse_format(&channel.api_format)?) {
            return Err(ConfigError::Compile(
                "API key channel access references a disallowed format".into(),
            ));
        }
    }
    positive_policy(record.requests_per_minute, "requests_per_minute")?;
    positive_policy(record.max_concurrent_requests, "max_concurrent_requests")?;
    if record
        .quota_limit_amount
        .is_some_and(|amount| amount.is_sign_negative())
        || record.quota_used_amount.is_sign_negative()
    {
        return Err(ConfigError::Compile(
            "API key quota amounts must be non-negative".into(),
        ));
    }
    Ok(())
}
fn positive_policy(value: Option<i32>, name: &str) -> Result<Option<u32>, ConfigError> {
    value
        .map(|value| {
            u32::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    ConfigError::Compile(format!("API key {name} must be positive when configured"))
                })
        })
        .transpose()
}
fn validate_rule(record: &ModelRuleRecord) -> Result<(), ConfigError> {
    require("model rule client_model", &record.client_model)?;
    require("model rule upstream_model", &record.upstream_model)?;
    parse_format(&record.api_format)?;
    unique(&record.channel_group_ids, "model rule channel_group_ids")?;
    unique(&record.channel_ids, "model rule channel_ids")?;
    if record.channel_group_ids.is_empty() && record.channel_ids.is_empty() {
        return Err(ConfigError::Compile(
            "model rule must select at least one target".into(),
        ));
    }
    Ok(())
}
fn compile_auth(channel: &ChannelRecord) -> Result<UpstreamAuth, ConfigError> {
    match channel.upstream_auth_kind.as_str() {
        "none"
            if channel.upstream_auth_header_name.is_none()
                && channel.upstream_api_key.is_none() =>
        {
            Ok(UpstreamAuth::None)
        }
        "bearer" if channel.upstream_auth_header_name.is_none() => Ok(UpstreamAuth::Bearer(
            secret_header(channel.upstream_api_key.as_deref())?,
        )),
        "header" => {
            let name = channel
                .upstream_auth_header_name
                .as_deref()
                .ok_or_else(|| {
                    ConfigError::Compile("header upstream auth requires a header name".into())
                })?;
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConfigError::Compile("invalid upstream auth header name".into()))?;
            if matches!(
                name.as_str(),
                "authorization"
                    | "host"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
                    | "proxy-authorization"
                    | "proxy-authenticate"
                    | "keep-alive"
                    | "te"
                    | "trailer"
                    | "upgrade"
                    | "proxy-connection"
            ) || client_header_explicitly_ignored(&name)
            {
                return Err(ConfigError::Compile(
                    "unsafe upstream auth header name".into(),
                ));
            }
            Ok(UpstreamAuth::Header {
                name,
                value: secret_header(channel.upstream_api_key.as_deref())?,
            })
        }
        _ => Err(ConfigError::Compile(
            "invalid upstream auth configuration".into(),
        )),
    }
}
fn secret_header(value: Option<&str>) -> Result<Arc<str>, ConfigError> {
    let value =
        value.ok_or_else(|| ConfigError::Compile("upstream auth requires credentials".into()))?;
    require("upstream auth credential", value)?;
    HeaderValue::from_str(value).map_err(|_| {
        ConfigError::Compile("upstream auth credential is not a valid HTTP header value".into())
    })?;
    Ok(Arc::from(value))
}
fn parse_format(value: &str) -> Result<ApiFormat, ConfigError> {
    ApiFormat::parse(value).ok_or_else(|| ConfigError::Compile("unsupported API format".into()))
}
fn parse_permission(value: &str) -> Result<ApiKeyPermission, ConfigError> {
    match value {
        "proxy" => Ok(ApiKeyPermission::Proxy),
        "models.read" => Ok(ApiKeyPermission::ModelsRead),
        _ => Err(ConfigError::Compile(
            "unsupported API key permission".into(),
        )),
    }
}
fn parse_url(id: Uuid, value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value)
        .map_err(|_| ConfigError::Compile(format!("channel {id} has an invalid base URL")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConfigError::Compile(
            "channel base URL must be an http(s) URL without credentials, query, or fragment"
                .into(),
        ));
    }
    Ok(url)
}
fn require(field: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        Err(ConfigError::Compile(format!("{field} must not be blank")))
    } else {
        Ok(())
    }
}
fn invalid_no_proxy_host() -> ConfigError {
    ConfigError::Compile("proxy no_proxy host pattern is invalid".into())
}
fn unique<T: Eq + std::hash::Hash>(items: &[T], field: &str) -> Result<(), ConfigError> {
    if items.iter().collect::<HashSet<_>>().len() != items.len() {
        Err(ConfigError::Compile(format!("duplicate {field} item")))
    } else {
        Ok(())
    }
}
fn insert_unique<T>(
    map: &mut HashMap<Uuid, T>,
    id: Uuid,
    value: T,
    field: &str,
) -> Result<(), ConfigError> {
    if map.insert(id, value).is_some() {
        Err(dup(field))
    } else {
        Ok(())
    }
}
fn dup(field: &str) -> ConfigError {
    ConfigError::Compile(format!("duplicate {field}"))
}
fn validate_server(server: &ServerConfig) -> Result<(), ConfigError> {
    require("server host", &server.host)?;
    if server.shutdown_grace_period_seconds == 0 {
        return Err(ConfigError::Compile(
            "server shutdown_grace_period_seconds must be greater than zero".into(),
        ));
    }
    Ok(())
}
fn validate_database(database: &DatabaseConfig) -> Result<(), ConfigError> {
    if database.max_connections == 0 || database.connect_timeout_seconds == 0 {
        return Err(ConfigError::Compile(
            "database limits must be greater than zero".into(),
        ));
    }
    let url = Url::parse(&database.url)
        .map_err(|_| ConfigError::Compile("database URL is invalid".into()))?;
    if url.scheme() != "postgres" && url.scheme() != "postgresql" {
        return Err(ConfigError::Compile(
            "database URL must use postgres".into(),
        ));
    }
    if database
        .password_file
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        return Err(ConfigError::Compile(
            "database password_file must not be empty".into(),
        ));
    }
    let url_contains_password = url.password().is_some()
        || url
            .query_pairs()
            .any(|(name, _)| name.eq_ignore_ascii_case("password"));
    if database.password_file.is_some() && url_contains_password {
        return Err(ConfigError::Compile(
            "database password must be configured in either url or password_file, not both".into(),
        ));
    }
    Ok(())
}
fn validate_upstream(upstream: &UpstreamConfig) -> Result<(), ConfigError> {
    if upstream.connect_timeout_seconds == 0
        || upstream.response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.images_response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.standalone_web_search_response_header_timeout_seconds
            <= upstream.connect_timeout_seconds
        || upstream.stream_idle_timeout_seconds == 0
    {
        return Err(ConfigError::Compile(
            "invalid upstream timeout settings".into(),
        ));
    }
    Ok(())
}
fn validate_automatic_disable_config(config: &AutomaticDisableConfig) -> Result<(), ConfigError> {
    if config
        .error_status_codes
        .iter()
        .any(|status| !(100..=599).contains(status))
        || config
            .error_message_keywords
            .iter()
            .any(|keyword| keyword.trim().is_empty() || keyword.chars().count() > 200)
    {
        return Err(ConfigError::Compile(
            "automatic_disable settings are invalid".into(),
        ));
    }
    Ok(())
}

fn validate_request_retry_config(config: &RequestRetryConfig) -> Result<(), ConfigError> {
    if config.max_retries == 0 || config.max_retries > MAX_REQUEST_RETRIES {
        return Err(ConfigError::Compile(
            "request_retry max_retries must be between 1 and 10".into(),
        ));
    }
    Ok(())
}
fn validate_scheduled_testing_config(config: &ScheduledTestingConfig) -> Result<(), ConfigError> {
    if config.interval_minutes == 0
        || config.prompt.trim().is_empty()
        || config.prompt.chars().count() > 4_000
        || !matches!(config.mode.as_str(), "global" | "failure_only")
    {
        return Err(ConfigError::Compile(
            "scheduled_testing settings are invalid".into(),
        ));
    }
    Ok(())
}
fn validate_session_affinity_config(config: &SessionAffinityConfig) -> Result<(), ConfigError> {
    compile_session_affinity_settings(&SystemSessionAffinitySettingsInput {
        enabled: config.enabled,
        max_entries: config.max_entries,
        default_ttl_seconds: config.default_ttl_seconds,
        rules: config.rules.clone(),
    })
    .map(|_| ())
}
fn validate_models_sync(config: &ModelsSyncConfig) -> Result<(), ConfigError> {
    let url = Url::parse(&config.api_url).map_err(|_| {
        ConfigError::Compile("models_sync api_url must be a valid HTTPS URL".into())
    })?;
    if url.scheme() != "https"
        || url.host().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || config.request_timeout_seconds == 0
        || config.max_response_bytes == 0
        || config.max_model_metadata_bytes == 0
        || config.max_selections == 0
    {
        return Err(ConfigError::Compile(
            "models_sync settings are invalid".into(),
        ));
    }
    Ok(())
}
fn validate_console(
    config: ConsoleFileConfig,
    auth: AuthFileConfig,
) -> Result<Option<ConsoleListenerConfig>, ConfigError> {
    if !config.enabled {
        return Ok(None);
    }
    #[cfg(not(feature = "embedded-console-ui"))]
    if config.ui_enabled {
        return Err(ConfigError::Compile(
            "console ui_enabled requires building with the embedded-console-ui cargo feature"
                .into(),
        ));
    }
    let host = config
        .host
        .ok_or_else(|| ConfigError::Compile("enabled console host is required".into()))?;
    let address = host
        .parse::<std::net::IpAddr>()
        .map_err(|_| ConfigError::Compile("console host must be an IP address".into()))?;
    let port = config
        .port
        .filter(|port| *port != 0)
        .ok_or_else(|| ConfigError::Compile("enabled console port is required".into()))?;
    let issuer = required_auth_value(auth.issuer, "auth issuer")?;
    let audience = required_auth_value(auth.audience, "auth audience")?;
    let key_id = required_auth_value(auth.key_id, "auth key_id")?;
    let signing_key_path = auth
        .signing_key_path
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            ConfigError::Compile("enabled console signing_key_path is required".into())
        })?;
    let verification_key_path = auth
        .verification_key_path
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            ConfigError::Compile("enabled console verification_key_path is required".into())
        })?;
    let access_token_ttl_seconds = auth
        .access_token_ttl_seconds
        .filter(|ttl| *ttl > 0)
        .ok_or_else(|| {
            ConfigError::Compile("enabled console access token TTL is required".into())
        })?;
    let refresh_token_ttl_seconds = auth
        .refresh_token_ttl_seconds
        .filter(|ttl| *ttl > access_token_ttl_seconds)
        .ok_or_else(|| {
            ConfigError::Compile(
                "console refresh token TTL must be greater than access token TTL".into(),
            )
        })?;
    for origin in &config.allowed_origins {
        validate_console_origin(origin)?;
    }
    unique(&config.allowed_origins, "console allowed origin")?;
    Ok(Some(ConsoleListenerConfig {
        address: SocketAddr::new(address, port),
        allowed_origins: config.allowed_origins,
        auth: AuthConfig {
            issuer,
            audience,
            access_token_ttl_seconds,
            refresh_token_ttl_seconds,
            key_id,
            signing_key_path,
            verification_key_path,
        },
        ui_enabled: config.ui_enabled,
    }))
}

fn validate_mcp(config: McpFileConfig) -> Result<SystemMcpSettingsInput, ConfigError> {
    let input = SystemMcpSettingsInput {
        enabled: config.enabled,
        public_base_url: config.public_base_url,
        allowed_origins: config.allowed_origins,
        allow_legacy_2025_11_25: config.allow_legacy_2025_11_25,
        request_body_bytes: config.request_body_bytes,
        image_request_body_bytes: config.image_request_body_bytes,
        search_result_bytes: config.search_result_bytes,
        image_result_bytes: config.image_result_bytes,
    };
    compile_mcp_transport_settings(&input)?;
    Ok(input)
}

fn required_auth_value(value: Option<String>, field: &str) -> Result<String, ConfigError> {
    let value = value
        .ok_or_else(|| ConfigError::Compile(format!("enabled console {field} is required")))?;
    require(field, &value)?;
    Ok(value)
}

fn validate_console_origin(origin: &str) -> Result<(), ConfigError> {
    validate_http_origin(origin, "console allowed origin")
}

fn validate_http_origin(origin: &str, field: &str) -> Result<(), ConfigError> {
    parse_http_origin(origin, field).map(|_| ())
}

fn canonical_http_origin(origin: &str, field: &str) -> Result<String, ConfigError> {
    Ok(parse_http_origin(origin, field)?
        .origin()
        .ascii_serialization())
}

fn parse_http_origin(origin: &str, field: &str) -> Result<Url, ConfigError> {
    let parsed =
        Url::parse(origin).map_err(|_| ConfigError::Compile(format!("{field} is invalid")))?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || origin == "*"
    {
        return Err(ConfigError::Compile(format!(
            "{field} must be an HTTP(S) origin without path or credentials"
        )));
    }
    Ok(parsed)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse TOML configuration file {path} (line {line:?}, column {column:?})")]
    Parse {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("failed to read database password file {path}")]
    DatabasePasswordRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid runtime configuration: {0}")]
    Compile(String),
}

#[cfg(test)]
mod tests {
    use crate::persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        McpServerRecord, ModelRecord, ModelRuleRecord, ProxyRecord, RuntimeConfigRecords,
        SystemPassiveHealthSettingsInput, SystemRequestRetrySettingsInput,
        SystemSessionAffinityKeySourceInput, SystemSessionAffinityRuleInput,
        SystemSessionAffinitySettingsInput, SystemSettingsInput, SystemSettingsRecord,
        SystemUpstreamSettingsInput,
    };

    use super::*;

    fn route_records(
        first_priority: i32,
        first_strategy: &str,
        second_priority: i32,
        second_strategy: &str,
        direct_duplicate: bool,
    ) -> ControlPlaneRecords {
        let first_group = Uuid::from_u128(1);
        let second_group = Uuid::from_u128(2);
        let first_channel = Uuid::from_u128(11);
        let second_channel = Uuid::from_u128(12);
        let group = |id, priority, strategy: &str| ChannelGroupRecord {
            id,
            name: id.to_string(),
            api_format: "open_ai_chat_completions".into(),
            connector_kind: "openai_compatible".into(),
            priority,
            selection_strategy: strategy.into(),
            enabled: true,
        };
        let channel = |id, group_id| ChannelRecord {
            id,
            channel_group_id: group_id,
            api_format: "open_ai_chat_completions".into(),
            name: id.to_string(),
            base_url: format!("https://{id}.test"),
            enabled: true,
            supports_websocket: false,
            supports_standalone_web_search: false,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: rust_decimal::Decimal::ONE,
            proxy_id: None,
            config_template_id: None,
            override_document: serde_json::json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "none".into(),
            upstream_auth_header_name: None,
            upstream_api_key: None,
            available_models: vec!["upstream".into()],
            test_model: None,
        };
        ControlPlaneRecords {
            api_keys: vec![],
            groups: vec![
                group(first_group, first_priority, first_strategy),
                group(second_group, second_priority, second_strategy),
            ],
            channels: vec![
                channel(first_channel, first_group),
                channel(second_channel, second_group),
            ],
            models: vec![],
            model_rules: vec![ModelRuleRecord {
                id: Uuid::from_u128(20),
                client_model: "client".into(),
                api_format: "open_ai_chat_completions".into(),
                upstream_model_id: Uuid::from_u128(21),
                upstream_model_enabled: true,
                upstream_model_currency: "USD".into(),
                price_unit_tokens: 1_000_000,
                price_effective_at: chrono::Utc::now(),
                input_unit_price: Default::default(),
                cached_input_unit_price: Default::default(),
                cache_write_unit_price: Default::default(),
                output_unit_price: Default::default(),
                advanced_billing: serde_json::json!({
                    "long_context_tiers": [],
                    "request_multipliers": [],
                }),
                upstream_model: "upstream".into(),
                channel_group_ids: vec![first_group, second_group],
                channel_ids: direct_duplicate
                    .then_some(first_channel)
                    .into_iter()
                    .collect(),
                enabled: true,
            }],
            proxies: vec![],
            templates: vec![],
            mcp_servers: vec![],
        }
    }

    #[test]
    fn compiler_rejects_non_usd_model_prices() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.model_rules[0].upstream_model_currency = "EUR".into();
        assert!(compile_control_plane(records).is_err());
    }

    #[test]
    fn compiler_rejects_request_json_transforms_on_standalone_search_channels() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        for group in &mut records.groups {
            group.api_format = "open_ai_responses".into();
        }
        for channel in &mut records.channels {
            channel.api_format = "open_ai_responses".into();
            channel.supports_standalone_web_search = true;
        }
        records.channels[0].override_document = serde_json::json!({
            "version": 1,
            "api_format": "open_ai_responses",
            "request_json": [
                {"op": "add", "path": "/settings/test", "value": true}
            ]
        });
        records.model_rules[0].api_format = "open_ai_responses".into();

        let error = compile_control_plane(records).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("standalone web search channels do not support request JSON transforms")
        );
    }

    #[test]
    fn compiler_registers_enabled_web_search_mcp_servers() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        for group in &mut records.groups {
            group.api_format = "open_ai_responses".into();
        }
        for channel in &mut records.channels {
            channel.api_format = "open_ai_responses".into();
            channel.supports_standalone_web_search = true;
        }
        records.model_rules[0].api_format = "open_ai_responses".into();
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(30),
            slug: "search".into(),
            kind: "web_search".into(),
            name: "Search".into(),
            description: None,
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({}),
            enabled: true,
        });
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(31),
            slug: "search-docs".into(),
            kind: "web_search".into(),
            name: "Documentation search".into(),
            description: Some("Search a separately managed domain policy.".into()),
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({
                "allowed_domains": ["Docs.Example.Test"]
            }),
            enabled: true,
        });

        let snapshot = compile_control_plane(records).unwrap();
        let server = snapshot.mcp_server("search").unwrap();
        let docs = snapshot.mcp_server("search-docs").unwrap();

        assert_eq!(server.name(), "Search");
        assert_eq!(server.model_rule().client_model(), "client");
        assert_eq!(docs.name(), "Documentation search");
        assert_eq!(
            docs.web_search_settings().unwrap().allowed_domains,
            ["docs.example.test"]
        );
    }

    #[test]
    fn compiler_registers_enabled_image_mcp_servers() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        for group in &mut records.groups {
            group.api_format = "open_ai_images".into();
        }
        for channel in &mut records.channels {
            channel.api_format = "open_ai_images".into();
        }
        records.model_rules[0].api_format = "open_ai_images".into();
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(30),
            slug: "image".into(),
            kind: "image".into(),
            name: "Image generation".into(),
            description: None,
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({
                "background": "opaque",
                "quality": "high",
                "size": "1536x1024"
            }),
            enabled: true,
        });

        let snapshot = compile_control_plane(records).unwrap();
        let server = snapshot.mcp_server("image").unwrap();
        let settings = server.image_settings().unwrap();

        assert_eq!(server.kind(), McpServerKind::Image);
        assert_eq!(server.model_rule().api_format(), ApiFormat::OpenAiImages);
        assert_eq!(settings.size, "1536x1024");
    }

    #[test]
    fn compiler_rejects_invalid_image_mcp_settings() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(30),
            slug: "image".into(),
            kind: "image".into(),
            name: "Image generation".into(),
            description: None,
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({"size": "00064x1024"}),
            enabled: false,
        });

        assert!(compile_control_plane(records).is_err());
    }

    #[test]
    fn compiler_rejects_invalid_web_search_mcp_settings() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(30),
            slug: "search".into(),
            kind: "web_search".into(),
            name: "Search".into(),
            description: None,
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({
                "max_output_tokens": {"short": 6000, "medium": 3000, "long": 1000}
            }),
            enabled: false,
        });

        assert!(compile_control_plane(records).is_err());
    }

    #[test]
    fn compiler_requires_search_capability_for_enabled_mcp_servers() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        for group in &mut records.groups {
            group.api_format = "open_ai_responses".into();
        }
        for channel in &mut records.channels {
            channel.api_format = "open_ai_responses".into();
        }
        records.model_rules[0].api_format = "open_ai_responses".into();
        records.mcp_servers.push(McpServerRecord {
            id: Uuid::from_u128(30),
            slug: "search".into(),
            kind: "web_search".into(),
            name: "Search".into(),
            description: None,
            model_rule_id: records.model_rules[0].id,
            settings_version: 1,
            settings: serde_json::json!({}),
            enabled: true,
        });

        let error = compile_control_plane(records).unwrap_err().to_string();
        assert!(error.contains("search-capable channel"));
    }

    #[test]
    #[cfg(feature = "mcp-server")]
    fn bootstrap_validates_mcp_public_origin_and_limits() {
        let input = validate_mcp(McpFileConfig {
            enabled: true,
            public_base_url: Some("https://MCP.example.test:443/".into()),
            allowed_origins: vec!["https://CLIENT.example.test/".into()],
            allow_legacy_2025_11_25: false,
            request_body_bytes: 1024,
            image_request_body_bytes: 3072,
            search_result_bytes: 2048,
            image_result_bytes: 4096,
        })
        .unwrap();
        let config = compile_mcp_transport_settings(&input).unwrap();

        assert_eq!(
            config.allowed_hosts(),
            ["mcp.example.test", "mcp.example.test:443"]
        );
        assert_eq!(config.public_base_url(), Some("https://mcp.example.test"));
        assert_eq!(config.allowed_origins(), ["https://client.example.test"]);
        assert_eq!(config.image_request_body_bytes(), 3072);
        assert_eq!(config.search_result_bytes(), 2048);
        assert_eq!(config.image_result_bytes(), 4096);

        assert!(
            validate_mcp(McpFileConfig {
                enabled: true,
                public_base_url: Some("https://mcp.example.test".into()),
                allowed_origins: vec![
                    "https://client.example.test".into(),
                    "https://CLIENT.example.test/".into(),
                ],
                allow_legacy_2025_11_25: false,
                request_body_bytes: 1024,
                image_request_body_bytes: 3072,
                search_result_bytes: 2048,
                image_result_bytes: 4096,
            })
            .is_err()
        );
        assert!(
            validate_mcp(McpFileConfig {
                enabled: true,
                public_base_url: Some("https://mcp.example.test".into()),
                allowed_origins: vec![],
                allow_legacy_2025_11_25: false,
                request_body_bytes: 1024,
                image_request_body_bytes: 3072,
                search_result_bytes: 2048,
                image_result_bytes: 64 * 1_024 * 1_024 + 1,
            })
            .is_err()
        );
        assert!(
            validate_mcp(McpFileConfig {
                enabled: true,
                public_base_url: Some("https://mcp.example.test".into()),
                allowed_origins: vec![],
                allow_legacy_2025_11_25: false,
                request_body_bytes: 1024,
                image_request_body_bytes: 64 * 1_024 * 1_024 + 1,
                search_result_bytes: 2048,
                image_result_bytes: 4096,
            })
            .is_err()
        );
    }

    #[test]
    #[cfg(not(feature = "mcp-server"))]
    fn bootstrap_rejects_mcp_without_the_cargo_feature() {
        assert!(
            validate_mcp(McpFileConfig {
                enabled: true,
                public_base_url: Some("https://mcp.example.test".into()),
                allowed_origins: vec![],
                allow_legacy_2025_11_25: false,
                request_body_bytes: 1024,
                image_request_body_bytes: 3072,
                search_result_bytes: 2048,
                image_result_bytes: 4096,
            })
            .is_err()
        );
    }

    #[test]
    fn compiler_rejects_forwarding_metadata_as_custom_auth_headers() {
        for name in ["forwarded", "x-forwarded-for", "cf-connecting-ip"] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.channels[0].upstream_auth_kind = "header".into();
            records.channels[0].upstream_auth_header_name = Some(name.into());
            records.channels[0].upstream_api_key = Some("credential".into());
            let error = compile_control_plane(records).unwrap_err().to_string();
            assert!(error.contains("unsafe upstream auth header name"), "{name}");
        }
    }

    #[test]
    fn compiler_requires_a_priced_model_for_each_scheduled_test_model() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.channels[0].test_model = Some("upstream".into());
        let error = compile_control_plane(records).unwrap_err().to_string();
        assert!(error.contains("configured priced model"));

        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.channels[0].test_model = Some("upstream".into());
        let model_id = records.model_rules[0].upstream_model_id;
        records.models.push(ModelRecord {
            id: model_id,
            source_model_id: "upstream".into(),
            currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: rust_decimal::Decimal::new(10, 2),
            cached_input_unit_price: rust_decimal::Decimal::new(5, 2),
            cache_write_unit_price: rust_decimal::Decimal::new(20, 2),
            output_unit_price: rust_decimal::Decimal::new(30, 2),
            advanced_billing: serde_json::json!({
                "long_context_tiers": [],
                "request_multipliers": [],
            }),
        });

        let snapshot = compile_control_plane(records).unwrap();
        let scheduled = snapshot.scheduled_test_model("upstream").unwrap();
        assert_eq!(scheduled.id(), model_id);
        assert_eq!(
            scheduled.price_snapshot().output_unit_price(),
            rust_decimal::Decimal::new(30, 2)
        );
    }

    #[test]
    fn compiler_rejects_scheduled_test_models_for_images_channels() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        for group in &mut records.groups {
            group.api_format = "open_ai_images".into();
        }
        for channel in &mut records.channels {
            channel.api_format = "open_ai_images".into();
        }
        records.model_rules[0].api_format = "open_ai_images".into();
        records.channels[0].test_model = Some("upstream".into());

        let error = compile_control_plane(records).unwrap_err().to_string();

        assert!(error.contains("Images channels do not support scheduled test models"));
    }

    #[test]
    fn bootstrap_rejects_dynamic_toml() {
        let value = "[server]\nhost='x'\nport=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=1\n[observability]\nfilter='info'\n[[api_keys]]\nid='bad'";
        assert!(toml::from_str::<AppConfig>(value).is_err());
    }

    #[test]
    fn bootstrap_rejects_removed_server_body_limit() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'";
        assert!(toml::from_str::<AppConfig>(value).is_err());
    }

    #[test]
    fn bootstrap_defaults_long_running_response_header_timeouts_for_older_toml() {
        let value = "[server]\nhost='x'\nport=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'";
        let config = toml::from_str::<AppConfig>(value).unwrap();

        assert_eq!(
            config.upstream.images_response_header_timeout_seconds,
            DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS
        );
        assert_eq!(
            config
                .upstream
                .standalone_web_search_response_header_timeout_seconds,
            DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS
        );
    }

    #[test]
    fn production_example_configuration_parses_and_validates() {
        let example = include_str!("../../config.example.toml");
        assert!(example.contains(r#"name = "session-id""#));
        assert!(example.contains(r#"name = "thread-id""#));
        assert!(example.contains(r#"pointer = "/id""#));
        assert!(!example.contains(r#"name = "session_id""#));
        assert!(!example.contains(r#"name = "thread_id""#));
        toml::from_str::<AppConfig>(example)
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn container_example_configuration_matches_the_embedded_image_contract() {
        let example = include_str!("../../deploy/compose/config.example.toml");
        assert!(example.contains(r#"name = "session-id""#));
        assert!(example.contains(r#"name = "thread-id""#));
        assert!(example.contains(r#"pointer = "/id""#));
        assert!(!example.contains(r#"name = "session_id""#));
        assert!(!example.contains(r#"name = "thread_id""#));
        let file = toml::from_str::<AppConfig>(example).unwrap();

        #[cfg(not(feature = "embedded-console-ui"))]
        assert!(file.validate().is_err());

        #[cfg(feature = "embedded-console-ui")]
        {
            let config = file.validate().unwrap();
            assert_eq!(config.server.host, "0.0.0.0");
            assert_eq!(
                config.database.url,
                "postgres://ai_gateway@postgres:5432/ai_gateway"
            );
            assert_eq!(
                config.request_logging.spool_directory,
                PathBuf::from("/var/lib/ai-gateway/request-log-spool")
            );
            assert_eq!(
                config.request_limits.image_edit_spool_directory,
                PathBuf::from("/var/lib/ai-gateway/image-edit-spool")
            );
            assert!(config.console.unwrap().ui_enabled);
        }
    }

    #[test]
    fn compiler_uses_database_backed_forwarding_settings() {
        let records = RuntimeConfigRecords {
            control_plane: route_records(0, "weighted_random", 1, "weighted_random", false),
            system_settings: SystemSettingsRecord {
                setting_key: FORWARDING_SETTINGS_KEY.into(),
                value: serde_json::to_value(SystemSettingsInput {
                    api_hosts: Vec::new(),
                    upstream: SystemUpstreamSettingsInput {
                        connect_timeout_seconds: 2,
                        response_header_timeout_seconds: 5,
                        images_response_header_timeout_seconds: 300,
                        standalone_web_search_response_header_timeout_seconds: 300,
                        stream_idle_timeout_seconds: 8,
                    },
                    request_retry: SystemRequestRetrySettingsInput {
                        enabled: true,
                        max_retries: 3,
                    },
                    passive_health: SystemPassiveHealthSettingsInput {
                        connection_failure_threshold: 4,
                        cooldown_seconds: 45,
                    },
                    automatic_disable: crate::persistence::SystemAutomaticDisableSettingsInput {
                        enabled: true,
                        error_status_codes: vec![429],
                        error_message_keywords: vec!["quota exceeded".into()],
                    },
                    scheduled_testing: crate::persistence::SystemScheduledTestingSettingsInput {
                        mode: "failure_only".into(),
                        auto_recover: false,
                        interval_minutes: 7,
                        prompt: "reply '1'".into(),
                    },
                    session_affinity: Default::default(),
                    websocket: Default::default(),
                    mcp: Default::default(),
                })
                .unwrap(),
                updated_at: chrono::Utc::now(),
            },
        };
        let snapshot = compile_runtime_config(records).unwrap();
        let settings = snapshot.system_settings();
        assert_eq!(
            settings.upstream_timeouts().connect(),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            settings.upstream_timeouts().response_header(),
            std::time::Duration::from_secs(5)
        );
        assert_eq!(
            settings.upstream_timeouts().images_response_header(),
            std::time::Duration::from_secs(300)
        );
        assert_eq!(
            settings
                .upstream_timeouts()
                .standalone_web_search_response_header(),
            std::time::Duration::from_secs(300)
        );
        assert!(settings.request_retry().enabled());
        assert_eq!(settings.request_retry().max_retries(), 3);
        assert_eq!(settings.passive_health().connection_failure_threshold(), 4);
        assert_eq!(
            settings.passive_health().cooldown(),
            std::time::Duration::from_secs(45)
        );
        assert!(settings.automatic_disable().matches_status(429));
        assert_eq!(
            settings.scheduled_testing().mode(),
            ScheduledTestingMode::FailureOnly
        );
        assert!(!settings.scheduled_testing().auto_recover());
        assert_eq!(
            settings.scheduled_testing().interval(),
            std::time::Duration::from_secs(7 * 60)
        );
    }

    #[test]
    fn compiler_prevalidates_session_affinity_rules() {
        let compiled = compile_system_settings_input(&SystemSettingsInput {
            api_hosts: Vec::new(),
            upstream: SystemUpstreamSettingsInput {
                connect_timeout_seconds: 1,
                response_header_timeout_seconds: 2,
                images_response_header_timeout_seconds: 300,
                standalone_web_search_response_header_timeout_seconds: 300,
                stream_idle_timeout_seconds: 3,
            },
            request_retry: Default::default(),
            passive_health: SystemPassiveHealthSettingsInput {
                connection_failure_threshold: 3,
                cooldown_seconds: 30,
            },
            automatic_disable: Default::default(),
            scheduled_testing: Default::default(),
            session_affinity: SystemSessionAffinitySettingsInput {
                enabled: true,
                max_entries: 100,
                default_ttl_seconds: 60,
                rules: vec![SystemSessionAffinityRuleInput {
                    name: "codex".into(),
                    enabled: true,
                    api_formats: vec!["open_ai_responses".into()],
                    model_regex: vec!["^gpt-.*$".into()],
                    key_sources: vec![
                        SystemSessionAffinityKeySourceInput::JsonPointer {
                            pointer: "/prompt_cache_key".into(),
                        },
                        SystemSessionAffinityKeySourceInput::RequestHeader {
                            name: "session_id".into(),
                        },
                        SystemSessionAffinityKeySourceInput::RequestHeader {
                            name: "thread_id".into(),
                        },
                    ],
                    value_regex: None,
                    ttl_seconds: None,
                }],
            },
            websocket: Default::default(),
            mcp: Default::default(),
        })
        .unwrap();

        assert!(compiled.session_affinity().enabled());
        assert_eq!(compiled.session_affinity().max_entries(), 100);
        assert_eq!(compiled.session_affinity().rules().len(), 1);
        assert_eq!(compiled.session_affinity().rules()[0].name(), "codex");
        assert_eq!(
            compiled.session_affinity().rules()[0].ttl(),
            std::time::Duration::from_secs(60)
        );
    }

    #[test]
    fn compiler_rejects_unsafe_session_affinity_sources() {
        let input = SystemSessionAffinitySettingsInput {
            enabled: true,
            max_entries: 100,
            default_ttl_seconds: 60,
            rules: vec![SystemSessionAffinityRuleInput {
                name: "unsafe".into(),
                enabled: true,
                api_formats: vec!["open_ai_responses".into()],
                model_regex: vec![],
                key_sources: vec![SystemSessionAffinityKeySourceInput::RequestHeader {
                    name: "Authorization".into(),
                }],
                value_regex: None,
                ttl_seconds: None,
            }],
        };

        assert!(compile_session_affinity_settings(&input).is_err());
    }

    #[test]
    fn compiler_rejects_session_affinity_headers_outside_the_client_allowlist() {
        let input = SystemSessionAffinitySettingsInput {
            enabled: true,
            max_entries: 100,
            default_ttl_seconds: 60,
            rules: vec![SystemSessionAffinityRuleInput {
                name: "unknown-header".into(),
                enabled: true,
                api_formats: vec!["open_ai_responses".into()],
                model_regex: vec![],
                key_sources: vec![SystemSessionAffinityKeySourceInput::RequestHeader {
                    name: "x-private-session".into(),
                }],
                value_regex: None,
                ttl_seconds: None,
            }],
        };

        let error = compile_session_affinity_settings(&input)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not allowed by the client request policy"));
    }

    #[test]
    fn compiler_rejects_images_session_affinity_rules() {
        let input = SystemSessionAffinitySettingsInput {
            enabled: true,
            max_entries: 100,
            default_ttl_seconds: 60,
            rules: vec![SystemSessionAffinityRuleInput {
                name: "images".into(),
                enabled: true,
                api_formats: vec!["open_ai_images".into()],
                model_regex: vec![],
                key_sources: vec![SystemSessionAffinityKeySourceInput::RequestHeader {
                    name: "x-session-id".into(),
                }],
                value_regex: None,
                ttl_seconds: None,
            }],
        };

        let error = compile_session_affinity_settings(&input)
            .unwrap_err()
            .to_string();

        assert!(error.contains("Images requests do not support session affinity"));
    }

    #[test]
    fn malformed_toml_error_never_retains_or_formats_raw_config() {
        let token = "fake-jwt-key-path-must-never-appear-in-an-error";
        let path = std::env::temp_dir().join(format!("ai-gateway-invalid-{}.toml", Uuid::new_v4()));
        std::fs::write(
            &path,
            format!("[auth]\nsigning_key_path = '{token}'\nnot valid toml"),
        )
        .unwrap();
        let error = match AppConfig::load(&path) {
            Err(error) => error,
            Ok(_) => panic!("malformed TOML unexpectedly parsed"),
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        std::fs::remove_file(path).unwrap();
        assert!(!display.contains(token));
        assert!(!debug.contains(token));
    }

    #[test]
    fn bootstrap_rejects_zero_shutdown_grace_period() {
        let value = "[server]\nhost='x'\nport=1\nshutdown_grace_period_seconds=0\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=1\n[observability]\nfilter='info'";
        assert!(
            toml::from_str::<AppConfig>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn bootstrap_defaults_stage_two_settings_when_absent() {
        let value = "[server]\nhost='x'\nport=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'";
        let config = toml::from_str::<AppConfig>(value)
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(config.server.shutdown_grace_period_seconds, 60);
        assert_eq!(config.request_logging.queue_capacity, 1_024);
        assert_eq!(config.request_logging.database_max_connections, 4);
        assert_eq!(config.request_logging.ingest_batch_size, 4_096);
        assert_eq!(config.request_logging.projection_batch_size, 2_048);
        assert_eq!(config.request_logging.settlement_batch_size, 4_096);
        assert_eq!(config.request_logging.settlement_interval_milliseconds, 500);
        assert_eq!(
            config.request_logging.spool_directory,
            PathBuf::from("./data/request-log-spool")
        );
        assert_eq!(
            config.request_logging.spool_compaction_threshold_bytes,
            256 * 1_024 * 1_024
        );
        assert!(config.request_retry.enabled);
        assert_eq!(config.request_retry.max_retries, 1);
        assert_eq!(config.passive_health.connection_failure_threshold, 3);
        assert_eq!(config.passive_health.cooldown_seconds, 30);
        assert!(!config.automatic_disable.enabled);
        assert!(config.automatic_disable.error_status_codes.is_empty());
        assert!(config.automatic_disable.error_message_keywords.is_empty());
        assert_eq!(config.scheduled_testing.mode, "global");
        assert!(config.scheduled_testing.auto_recover);
        assert_eq!(config.scheduled_testing.interval_minutes, 5);
        assert_eq!(config.scheduled_testing.prompt, "reply '1'");
        assert!(!config.session_affinity.enabled);
        assert_eq!(config.session_affinity.max_entries, 100_000);
        assert_eq!(config.session_affinity.default_ttl_seconds, 3_600);
        assert!(config.session_affinity.rules.is_empty());
        assert_eq!(config.request_limits.proxy_body_bytes, 1_048_576);
        assert_eq!(
            config.request_limits.image_edit_body_bytes,
            64 * 1_024 * 1_024
        );
        assert_eq!(
            config.request_limits.image_edit_file_bytes,
            50 * 1_024 * 1_024
        );
        assert_eq!(config.request_limits.image_edit_memory_bytes, 1_048_576);
        assert_eq!(
            config.request_limits.image_edit_spool_directory,
            PathBuf::from("./data/image-edit-spool")
        );
        assert_eq!(config.request_limits.console_body_bytes, 262_144);
        assert_eq!(config.request_limits.auth_body_bytes, 16_384);
    }

    #[test]
    fn bootstrap_rejects_incoherent_image_edit_body_limits() {
        let invalid_memory = RequestLimitsFileConfig {
            image_edit_body_bytes: 1_024,
            image_edit_file_bytes: 1_024,
            image_edit_memory_bytes: 2_048,
            ..Default::default()
        };
        assert!(RequestLimitsConfig::resolve(invalid_memory).is_err());

        let invalid_file = RequestLimitsFileConfig {
            image_edit_body_bytes: 1_024,
            image_edit_file_bytes: 2_048,
            image_edit_memory_bytes: 1_024,
            ..Default::default()
        };
        assert!(RequestLimitsConfig::resolve(invalid_file).is_err());

        let empty_directory = RequestLimitsFileConfig {
            image_edit_spool_directory: PathBuf::new(),
            ..Default::default()
        };
        assert!(RequestLimitsConfig::resolve(empty_directory).is_err());
    }

    #[test]
    fn bootstrap_preserves_ipv6_console_socket_address() {
        let value = "[server]\nhost='127.0.0.1'\nport=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'\n[console]\nenabled=true\nhost='::1'\nport=9443\nallowed_origins=['https://console.example.test']\n[auth]\nissuer='ai-gateway'\naudience='ai-gateway-console'\naccess_token_ttl_seconds=900\nrefresh_token_ttl_seconds=3600\nkey_id='test'\nsigning_key_path='/tmp/private.pem'\nverification_key_path='/tmp/public.pem'";
        let config = toml::from_str::<AppConfig>(value)
            .unwrap()
            .validate()
            .unwrap();

        assert_eq!(
            config.console.unwrap().address,
            std::net::SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), 9443)
        );
    }

    #[test]
    fn bootstrap_rejects_zero_request_log_queue_capacity() {
        let value = "[server]\nhost='x'\nport=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=0\n[observability]\nfilter='info'";
        assert!(
            toml::from_str::<AppConfig>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn database_password_file_is_applied_without_exposing_it_in_toml() {
        let path =
            std::env::temp_dir().join(format!("ai-gateway-database-password-{}", Uuid::new_v4()));
        std::fs::write(&path, "production-secret\r\n").unwrap();
        let config = DatabaseConfig {
            url: "postgres://user@127.0.0.1/database".into(),
            password_file: Some(path.clone()),
            max_connections: 1,
            connect_timeout_seconds: 1,
        };

        validate_database(&config).unwrap();
        config.connect_options().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn database_password_file_cannot_compete_with_url_password() {
        let config = DatabaseConfig {
            url: "postgres://user:inline-secret@127.0.0.1/database".into(),
            password_file: Some(PathBuf::from("/run/secrets/database-password")),
            max_connections: 1,
            connect_timeout_seconds: 1,
        };

        assert!(validate_database(&config).is_err());
    }

    #[test]
    fn database_password_file_cannot_compete_with_query_password() {
        let config = DatabaseConfig {
            url: "postgres://user@127.0.0.1/database?password=inline-secret".into(),
            password_file: Some(PathBuf::from("/run/secrets/database-password")),
            max_connections: 1,
            connect_timeout_seconds: 1,
        };

        assert!(validate_database(&config).is_err());
    }

    #[test]
    fn compiler_builds_sorted_priority_tiers() {
        let snapshot = compile_control_plane(route_records(
            10,
            "weighted_random",
            2,
            "weighted_random",
            false,
        ))
        .unwrap();
        let rule = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "client")
            .unwrap();
        assert_eq!(rule.tiers().len(), 2);
        assert_eq!(rule.tiers()[0].priority(), 2);
        assert_eq!(rule.tiers()[1].priority(), 10);
    }

    #[test]
    fn compiler_rejects_strategy_mismatch_in_any_priority_tier() {
        assert!(
            compile_control_plane(route_records(
                0,
                "weighted_random",
                0,
                "weighted_round_robin",
                false,
            ))
            .is_err()
        );
    }

    #[test]
    fn compiler_deduplicates_direct_candidate_already_reached_through_group() {
        let snapshot = compile_control_plane(route_records(
            0,
            "weighted_random",
            1,
            "weighted_random",
            true,
        ))
        .unwrap();
        let rule = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "client")
            .unwrap();
        assert_eq!(
            rule.tiers()
                .iter()
                .map(|tier| tier.channel_ids().len())
                .sum::<usize>(),
            2
        );
    }

    #[test]
    fn compiler_deduplicates_authorization_profiles_and_precomputes_accessible_routes() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        let first_group = records.groups[0].id;
        let second_group = records.groups[1].id;
        let first_channel = records.channels[0].id;
        records.model_rules[0].channel_group_ids = vec![first_group];
        let key = |id: u128, secret: &str, groups, channels| ApiKeyRecord {
            id: Uuid::from_u128(id),
            user_id: Uuid::from_u128(id + 100),
            user_status: "active".into(),
            user_websocket_enabled: false,
            secret_value: secret.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec!["open_ai_chat_completions".into()],
            permissions: vec!["proxy".into(), "models.read".into()],
            allowed_group_ids: groups,
            allowed_channel_ids: channels,
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        };
        records.api_keys = vec![
            key(30, "group-key", vec![first_group], vec![]),
            key(31, "channel-key", vec![], vec![first_channel]),
            key(32, "inaccessible-key", vec![second_group], vec![]),
        ];

        let snapshot = compile_control_plane(records).unwrap();
        let group_key = snapshot.authenticate("group-key").unwrap();
        let channel_key = snapshot.authenticate("channel-key").unwrap();
        let inaccessible_key = snapshot.authenticate("inaccessible-key").unwrap();
        assert!(group_key.shares_authorization_profile(&channel_key));
        assert!(!group_key.shares_authorization_profile(&inaccessible_key));
        let rule = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "client")
            .unwrap();
        assert!(group_key.permits_route(rule.route_slot()));
        assert!(!inaccessible_key.permits_route(rule.route_slot()));
        assert_eq!(
            snapshot.models_for(&group_key, ApiFormat::OpenAiChatCompletions),
            vec![Arc::from("client")]
        );
        assert!(
            snapshot
                .models_for(&inaccessible_key, ApiFormat::OpenAiChatCompletions)
                .is_empty()
        );
    }

    #[test]
    fn group_targets_filter_channels_by_upstream_model() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        let shared_group = records.groups[0].id;
        let first_channel = records.channels[0].id;
        let second_channel = records.channels[1].id;
        records.channels[1].channel_group_id = shared_group;
        records.channels[0].available_models = vec!["upstream-a".into()];
        records.channels[1].available_models = vec!["upstream-b".into()];
        records.model_rules[0].client_model = "client-a".into();
        records.model_rules[0].upstream_model = "upstream-a".into();
        records.model_rules[0].channel_group_ids = vec![shared_group];
        records.model_rules.push(ModelRuleRecord {
            id: Uuid::from_u128(22),
            client_model: "client-b".into(),
            api_format: "open_ai_chat_completions".into(),
            upstream_model_id: Uuid::from_u128(23),
            upstream_model_enabled: true,
            upstream_model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Default::default(),
            cached_input_unit_price: Default::default(),
            cache_write_unit_price: Default::default(),
            output_unit_price: Default::default(),
            advanced_billing: serde_json::json!({
                "long_context_tiers": [],
                "request_multipliers": [],
            }),
            upstream_model: "upstream-b".into(),
            channel_group_ids: vec![shared_group],
            channel_ids: vec![],
            enabled: true,
        });

        let snapshot = compile_control_plane(records).unwrap();
        assert_eq!(
            snapshot
                .model_rule(ApiFormat::OpenAiChatCompletions, "client-a")
                .unwrap()
                .tiers()[0]
                .channel_ids(),
            &[first_channel]
        );
        assert_eq!(
            snapshot
                .model_rule(ApiFormat::OpenAiChatCompletions, "client-b")
                .unwrap()
                .tiers()[0]
                .channel_ids(),
            &[second_channel]
        );
    }

    #[test]
    fn compiler_keeps_disabled_model_capable_channels_as_unavailable() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        let group_id = records.groups[0].id;
        let channel_id = records.channels[0].id;
        records.channels[0].enabled = false;
        records.model_rules[0].channel_group_ids = vec![group_id];

        let snapshot = compile_control_plane(records).unwrap();
        let rule = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "client")
            .unwrap();
        assert!(rule.tiers().is_empty());
        assert_eq!(rule.unavailable_candidates().len(), 1);
        assert_eq!(rule.unavailable_candidates()[0].channel_id(), channel_id);
    }

    #[test]
    fn compiler_rejects_model_incompatible_direct_channel() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        let channel_id = records.channels[0].id;
        records.channels[0].available_models = vec!["different-upstream".into()];
        records.model_rules[0].channel_group_ids.clear();
        records.model_rules[0].channel_ids = vec![channel_id];

        let error = compile_control_plane(records).unwrap_err().to_string();
        assert!(error.contains("direct channel candidate does not support"));
    }

    #[test]
    fn compiler_rejects_nonempty_channel_documents_even_when_disabled() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.channels[0].enabled = false;
        records.channels[0].override_document = serde_json::json!({
            "headers": {"Authorization": "must-not-be-accepted"}
        });

        assert!(compile_control_plane(records).is_err());
    }

    fn proxy(id: Uuid, url: &str, enabled: bool) -> ProxyRecord {
        ProxyRecord {
            id,
            name: "egress".into(),
            proxy_url: url.into(),
            username: Some("proxy-user".into()),
            password: Some("proxy-password".into()),
            no_proxy_hosts: vec!["internal.test".into()],
            enabled,
        }
    }

    fn template(id: Uuid, format: &str, enabled: bool) -> ConfigTemplateRecord {
        ConfigTemplateRecord {
            id,
            name: "defaults".into(),
            description: None,
            document: serde_json::json!({
                "version": 1,
                "api_format": format,
                "request_headers": {"set": {"x-template": "template-default"}}
            }),
            enabled,
        }
    }

    #[test]
    fn compiler_validates_proxy_schemes_no_proxy_hosts_and_never_leaks_credentials() {
        for scheme in ["http", "https", "socks5", "socks5h"] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            let id = Uuid::new_v4();
            records.proxies = vec![proxy(id, &format!("{scheme}://proxy.test:1080"), true)];
            records.channels[0].proxy_id = Some(id);
            assert!(
                compile_control_plane(records).is_ok(),
                "{scheme} should compile"
            );
        }

        for scheme in ["socks4", "socks4a"] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            let id = Uuid::new_v4();
            records.proxies = vec![ProxyRecord {
                username: None,
                password: None,
                ..proxy(id, &format!("{scheme}://proxy.test:1080"), true)
            }];
            records.channels[0].proxy_id = Some(id);
            assert!(
                compile_control_plane(records).is_ok(),
                "{scheme} without credentials should compile"
            );
        }

        let mut invalid_scheme = route_records(0, "weighted_random", 1, "weighted_random", false);
        let id = Uuid::new_v4();
        invalid_scheme.proxies = vec![proxy(id, "ftp://proxy.test", true)];
        invalid_scheme.channels[0].proxy_id = Some(id);
        let error = compile_control_plane(invalid_scheme)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("proxy-password"));
        assert!(!error.contains("proxy-user"));

        for hosts in [
            vec![" ".into()],
            vec!["same.test".into(), "same.test".into()],
        ] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.proxies = vec![ProxyRecord {
                no_proxy_hosts: hosts,
                ..proxy(Uuid::new_v4(), "https://proxy.test", true)
            }];
            assert!(compile_control_plane(records).is_err());
        }
    }

    #[test]
    fn compiler_rejects_invalid_socks_credentials_without_leaking_them() {
        const USERNAME: &str = "socks-credential-username-sentinel";
        const PASSWORD: &str = "socks-credential-password-sentinel";

        let compile_proxy = |scheme: &str, username: Option<String>, password: Option<String>| {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.proxies = vec![ProxyRecord {
                username,
                password,
                ..proxy(Uuid::new_v4(), &format!("{scheme}://proxy.test:1080"), true)
            }];
            compile_control_plane(records)
        };

        for scheme in ["socks4", "socks4a"] {
            for (username, password) in [
                (Some(USERNAME.to_owned()), None),
                (None, Some(PASSWORD.to_owned())),
                (Some(USERNAME.to_owned()), Some(PASSWORD.to_owned())),
            ] {
                let error = compile_proxy(scheme, username, password).unwrap_err();
                let rendered = format!("{error:?} {error}");
                assert!(!rendered.contains(USERNAME));
                assert!(!rendered.contains(PASSWORD));
            }
        }

        for scheme in ["socks5", "socks5h"] {
            assert!(compile_proxy(scheme, None, None).is_ok());
            assert!(compile_proxy(scheme, Some("u".repeat(255)), Some("p".repeat(255)),).is_ok());

            for (username, password) in [
                (Some(USERNAME.to_owned()), None),
                (None, Some(PASSWORD.to_owned())),
                (Some("é".repeat(128)), Some(PASSWORD.to_owned())),
                (Some(USERNAME.to_owned()), Some("é".repeat(128))),
            ] {
                let error = compile_proxy(scheme, username, password).unwrap_err();
                let rendered = format!("{error:?} {error}");
                assert!(!rendered.contains(USERNAME));
                assert!(!rendered.contains(PASSWORD));
            }
        }
    }

    #[test]
    fn compiler_validates_channel_resources_timeouts_and_template_composition() {
        let proxy_id = Uuid::new_v4();
        let template_id = Uuid::new_v4();
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.proxies = vec![proxy(proxy_id, "https://proxy.test", true)];
        records.templates = vec![template(template_id, "open_ai_chat_completions", true)];
        records.channels[0].proxy_id = Some(proxy_id);
        records.channels[0].config_template_id = Some(template_id);
        records.channels[0].override_document = serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-channel": "channel-override"}}
        });
        records.channels[0].connect_timeout_ms = Some(10);
        records.channels[0].response_header_timeout_ms = Some(20);
        records.channels[0].stream_idle_timeout_ms = Some(30);
        let channel_id = records.channels[0].id;

        let snapshot = compile_control_plane(records).unwrap();
        let channel = snapshot.channel(channel_id).unwrap();
        let policy = channel.upstream_policy();
        assert_eq!(policy.proxy().unwrap().id(), proxy_id);
        assert_eq!(policy.template().unwrap().id(), template_id);
        assert_eq!(
            policy.timeouts().connect(),
            Some(std::time::Duration::from_millis(10))
        );
        assert_eq!(
            policy.timeouts().response_header(),
            Some(std::time::Duration::from_millis(20))
        );
        assert_eq!(
            policy.timeouts().stream_idle(),
            Some(std::time::Duration::from_millis(30))
        );
        assert_eq!(
            policy
                .effective_transforms()
                .request_headers()
                .operations()
                .len(),
            2
        );

        let invalid_records = [
            (Some(Uuid::new_v4()), None, None, None),
            (Some(proxy_id), None, None, Some(false)),
            (None, Some(Uuid::new_v4()), None, None),
            (None, Some(template_id), None, Some(false)),
            (None, Some(template_id), Some(0), None),
        ];
        for (missing_proxy, template_reference, timeout, disabled) in invalid_records {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            let proxy_enabled = disabled.unwrap_or(true);
            records.proxies = vec![proxy(proxy_id, "https://proxy.test", proxy_enabled)];
            records.templates = vec![template(
                template_id,
                "open_ai_responses",
                disabled.unwrap_or(true),
            )];
            records.channels[0].proxy_id = missing_proxy;
            records.channels[0].config_template_id = template_reference;
            records.channels[0].connect_timeout_ms = timeout;
            let error = compile_control_plane(records).unwrap_err().to_string();
            assert!(!error.contains("proxy-password"));
            assert!(!error.contains("template-default"));
        }
    }

    #[test]
    fn compiler_validates_disabled_resources_without_leaking_record_values() {
        let url_user = "sentinel-proxy-url-user";
        let url_password = "sentinel-proxy-url-password";
        let document_value = "sentinel-disabled-template-value";

        let mut invalid_proxy = route_records(0, "weighted_random", 1, "weighted_random", false);
        invalid_proxy.proxies = vec![proxy(
            Uuid::new_v4(),
            &format!("https://{url_user}:{url_password}@proxy.test"),
            false,
        )];
        let proxy_error = compile_control_plane(invalid_proxy).unwrap_err();
        let proxy_rendered = format!("{proxy_error:?} {proxy_error}");
        assert!(!proxy_rendered.contains(url_user));
        assert!(!proxy_rendered.contains(url_password));

        let mut invalid_template = route_records(0, "weighted_random", 1, "weighted_random", false);
        invalid_template.templates = vec![ConfigTemplateRecord {
            document: serde_json::json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "unknown": document_value
            }),
            ..template(Uuid::new_v4(), "open_ai_chat_completions", false)
        }];
        let template_error = compile_control_plane(invalid_template).unwrap_err();
        let template_rendered = format!("{template_error:?} {template_error}");
        assert!(!template_rendered.contains(document_value));
    }

    #[test]
    fn compiler_validates_resources_referenced_by_disabled_and_auto_disabled_channels() {
        let cases = [
            (
                false,
                false,
                None,
                None,
                Some(Uuid::new_v4()),
                None,
                "missing proxy",
            ),
            (
                true,
                true,
                Some(false),
                None,
                Some(Uuid::new_v4()),
                None,
                "disabled proxy",
            ),
            (
                false,
                false,
                None,
                None,
                None,
                Some(Uuid::new_v4()),
                "missing template",
            ),
            (
                true,
                true,
                None,
                Some(false),
                None,
                Some(Uuid::new_v4()),
                "disabled template",
            ),
        ];
        for (
            enabled,
            auto_disabled,
            proxy_enabled,
            template_enabled,
            proxy_id,
            template_id,
            label,
        ) in cases
        {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.channels[0].enabled = enabled;
            records.channels[0].auto_disabled = auto_disabled;
            records.channels[0].proxy_id = proxy_id;
            records.channels[0].config_template_id = template_id;
            if let Some(proxy_enabled) = proxy_enabled {
                records.proxies = vec![proxy(
                    proxy_id.unwrap(),
                    "https://proxy.test",
                    proxy_enabled,
                )];
            }
            if let Some(template_enabled) = template_enabled {
                records.templates = vec![template(
                    template_id.unwrap(),
                    "open_ai_chat_completions",
                    template_enabled,
                )];
            }
            assert!(
                compile_control_plane(records).is_err(),
                "{label} was accepted"
            );
        }

        let mut cross_format = route_records(0, "weighted_random", 1, "weighted_random", false);
        cross_format.channels[0].enabled = false;
        let template_id = Uuid::new_v4();
        cross_format.channels[0].config_template_id = Some(template_id);
        cross_format.templates = vec![template(template_id, "open_ai_responses", true)];
        assert!(compile_control_plane(cross_format).is_err());
    }

    #[test]
    fn no_proxy_hosts_normalize_and_match_only_the_accepted_grammar() {
        let exact = NoProxyHost::parse("API.Example.Test").unwrap();
        assert_eq!(exact.dns_name(), Some("api.example.test"));
        assert!(exact.matches_host("api.example.test"));
        assert!(!exact.matches_host("sub.api.example.test"));

        let ipv4 = NoProxyHost::parse("192.0.2.1").unwrap();
        assert_eq!(ipv4.ip_addr(), Some("192.0.2.1".parse().unwrap()));
        assert!(ipv4.matches_host("192.0.2.1"));
        assert!(!ipv4.matches_host("192.0.2.2"));
        let ipv6 = NoProxyHost::parse("::1").unwrap();
        assert!(ipv6.matches_host("::1"));

        let suffix = NoProxyHost::parse("*.Example.Test").unwrap();
        assert!(suffix.is_dns_suffix());
        assert_eq!(suffix.dns_name(), Some("example.test"));
        assert!(suffix.matches_host("api.example.test"));
        assert!(suffix.matches_host("deep.api.example.test"));
        assert!(!suffix.matches_host("example.test"));
        assert!(!suffix.matches_host("other-example.test"));

        for malformed in [
            "sentinel malformed pattern",
            "api.example.test:443",
            "http://api.example.test",
            "*",
            "*.bad_underscore.test",
            "999.0.0.1",
            "api..example.test",
            "api.example.test.",
            "api*example.test",
        ] {
            let error = NoProxyHost::parse(malformed).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains(malformed));
        }
    }
}
