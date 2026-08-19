//! Validated process-wide forwarding and channel-automation settings carried
//! by runtime snapshots.

use std::{sync::Arc, time::Duration};

use regex::Regex;
use reqwest::header::HeaderName;

use super::{ApiFormat, ApiOperation};

/// Hard ceiling for one client request's automatic failover retries.
pub const MAX_REQUEST_RETRIES: u32 = 10;

/// Default Images response-header timeout for newly bootstrapped settings.
pub const DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS: u64 = 300;
/// Default standalone web-search response-header timeout for newly bootstrapped settings.
pub const DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS: u64 = 300;
/// Default privacy-preserving workspace path projected into Codex request metadata.
pub const DEFAULT_CODEX_WORKSPACE_PATH: &str = "/workspace";
/// Default privacy-preserving Git remote projected into Codex request metadata.
pub const DEFAULT_CODEX_GIT_REMOTE_URL: &str = "https://github.com/oai404iao/ai_gateway";
/// Default connector-owned Codex request originator.
pub const DEFAULT_CODEX_ORIGINATOR: &str = "codex_cli_rs";
/// Default connector-owned Codex client version.
pub const DEFAULT_CODEX_CLIENT_VERSION: &str = "0.146.0";
/// Default connector-owned Codex User-Agent.
///
/// Administrators can set a full native Codex CLI User-Agent, including its
/// platform and terminal suffix, in database-backed system settings.
pub const DEFAULT_CODEX_USER_AGENT: &str = "codex_cli_rs/0.146.0";
/// Default MCP JSON-RPC request envelope limit for Search endpoints.
pub const DEFAULT_MCP_REQUEST_BODY_BYTES: usize = 4 * 1_024 * 1_024;
/// Default MCP JSON-RPC request envelope limit for Images endpoints.
pub const DEFAULT_MCP_IMAGE_REQUEST_BODY_BYTES: usize = 32 * 1_024 * 1_024;
/// Default bounded result size for Search MCP calls.
pub const DEFAULT_MCP_SEARCH_RESULT_BYTES: usize = 4 * 1_024 * 1_024;
/// Default bounded result size for Images MCP calls.
pub const DEFAULT_MCP_IMAGE_RESULT_BYTES: usize = 32 * 1_024 * 1_024;
/// Hard limit for Images MCP request envelopes and collected results.
pub const MAX_MCP_IMAGE_BYTES: usize = 64 * 1_024 * 1_024;

/// Global timeout defaults used when a channel has no explicit override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpstreamTimeoutDefaults {
    connect: Duration,
    response_header: Duration,
    images_response_header: Duration,
    standalone_web_search_response_header: Duration,
    stream_idle: Duration,
}

impl UpstreamTimeoutDefaults {
    #[must_use]
    pub const fn new(connect: Duration, response_header: Duration, stream_idle: Duration) -> Self {
        Self {
            connect,
            response_header,
            images_response_header: response_header,
            standalone_web_search_response_header: response_header,
            stream_idle,
        }
    }

    #[must_use]
    pub const fn with_images_response_header(mut self, images_response_header: Duration) -> Self {
        self.images_response_header = images_response_header;
        self
    }

    #[must_use]
    pub const fn with_standalone_web_search_response_header(
        mut self,
        response_header: Duration,
    ) -> Self {
        self.standalone_web_search_response_header = response_header;
        self
    }

    #[must_use]
    pub const fn connect(self) -> Duration {
        self.connect
    }

    #[must_use]
    pub const fn response_header(self) -> Duration {
        self.response_header
    }

    #[must_use]
    pub const fn images_response_header(self) -> Duration {
        self.images_response_header
    }

    #[must_use]
    pub const fn standalone_web_search_response_header(self) -> Duration {
        self.standalone_web_search_response_header
    }

    #[must_use]
    pub const fn response_header_for(self, api_format: ApiFormat) -> Duration {
        match api_format {
            ApiFormat::OpenAiImages => self.images_response_header,
            ApiFormat::OpenAiChatCompletions | ApiFormat::OpenAiResponses => self.response_header,
        }
    }

    #[must_use]
    pub const fn response_header_for_operation(self, operation: ApiOperation) -> Duration {
        match operation {
            ApiOperation::StandaloneWebSearch => self.standalone_web_search_response_header,
            ApiOperation::ChatCompletions
            | ApiOperation::Responses
            | ApiOperation::ImagesGeneration
            | ApiOperation::ImagesEdit => self.response_header_for(operation.api_format()),
        }
    }

    #[must_use]
    pub const fn stream_idle(self) -> Duration {
        self.stream_idle
    }
}

impl Default for UpstreamTimeoutDefaults {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(10),
            Duration::from_secs(30),
            Duration::from_secs(90),
        )
        .with_images_response_header(Duration::from_secs(
            DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS,
        ))
        .with_standalone_web_search_response_header(Duration::from_secs(
            DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS,
        ))
    }
}

/// Process-wide passive connection-health settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PassiveHealthSettings {
    connection_failure_threshold: u32,
    cooldown: Duration,
}

impl PassiveHealthSettings {
    #[must_use]
    pub const fn new(connection_failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            connection_failure_threshold,
            cooldown,
        }
    }

    #[must_use]
    pub const fn connection_failure_threshold(self) -> u32 {
        self.connection_failure_threshold
    }

    #[must_use]
    pub const fn cooldown(self) -> Duration {
        self.cooldown
    }
}

impl Default for PassiveHealthSettings {
    fn default() -> Self {
        Self::new(3, Duration::from_secs(30))
    }
}

/// Immutable policy for retrying one client request on distinct channels
/// before any upstream response headers are received.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestRetrySettings {
    enabled: bool,
    max_retries: u32,
}

impl RequestRetrySettings {
    #[must_use]
    pub const fn new(enabled: bool, max_retries: u32) -> Self {
        Self {
            enabled,
            max_retries,
        }
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    /// Automatic retries after the initial upstream attempt.
    #[must_use]
    pub const fn max_retries(self) -> u32 {
        self.max_retries
    }
}

impl Default for RequestRetrySettings {
    fn default() -> Self {
        Self::new(true, 1)
    }
}

/// Immutable matching policy for automatically taking a channel out of
/// rotation after a configured upstream error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomaticDisableSettings {
    enabled: bool,
    error_status_codes: Arc<[u16]>,
    error_message_keywords: Arc<[Arc<str>]>,
}

impl AutomaticDisableSettings {
    #[must_use]
    pub fn new(
        enabled: bool,
        error_status_codes: Arc<[u16]>,
        error_message_keywords: Arc<[Arc<str>]>,
    ) -> Self {
        Self {
            enabled,
            error_status_codes,
            error_message_keywords,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn matches_status(&self, status: u16) -> bool {
        self.enabled && self.error_status_codes.contains(&status)
    }

    #[must_use]
    pub fn error_message_keywords(&self) -> &[Arc<str>] {
        &self.error_message_keywords
    }
}

impl Default for AutomaticDisableSettings {
    fn default() -> Self {
        Self::new(false, Arc::from([]), Arc::from([]))
    }
}

/// The sanitized upstream failure fact that can trigger automatic disabling.
///
/// It intentionally carries only a HTTP status or a configured keyword, never
/// the raw upstream response body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomaticDisableTrigger {
    HttpStatus(u16),
    ErrorMessageKeyword(Arc<str>),
}

impl AutomaticDisableSettings {
    #[must_use]
    pub fn matches_trigger(&self, trigger: &AutomaticDisableTrigger) -> bool {
        if !self.enabled {
            return false;
        }
        match trigger {
            AutomaticDisableTrigger::HttpStatus(status) => self.error_status_codes.contains(status),
            AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => self
                .error_message_keywords
                .iter()
                .any(|candidate| candidate.to_lowercase() == keyword.to_lowercase()),
        }
    }
}

/// Scope for periodic upstream test requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledTestingMode {
    Global,
    FailureOnly,
}

/// Immutable policy controlling periodic direct upstream test requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledTestingSettings {
    mode: ScheduledTestingMode,
    auto_recover: bool,
    interval: Duration,
    prompt: Arc<str>,
}

impl ScheduledTestingSettings {
    #[must_use]
    pub fn new(
        mode: ScheduledTestingMode,
        auto_recover: bool,
        interval: Duration,
        prompt: Arc<str>,
    ) -> Self {
        Self {
            mode,
            auto_recover,
            interval,
            prompt,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> ScheduledTestingMode {
        self.mode
    }

    #[must_use]
    pub const fn auto_recover(&self) -> bool {
        self.auto_recover
    }

    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

impl Default for ScheduledTestingSettings {
    fn default() -> Self {
        Self::new(
            ScheduledTestingMode::Global,
            true,
            Duration::from_secs(5 * 60),
            Arc::from("reply '1'"),
        )
    }
}

/// A compiled source used to extract a bounded session-affinity value from an
/// authenticated proxy request.
#[derive(Clone, Debug)]
pub enum SessionAffinityKeySource {
    RequestHeader(HeaderName),
    JsonPointer(Arc<str>),
}

/// One immutable, prevalidated session-affinity rule.
#[derive(Clone, Debug)]
pub struct SessionAffinityRule {
    name: Arc<str>,
    fingerprint: [u8; 32],
    api_formats: Arc<[ApiFormat]>,
    model_regex: Arc<[Regex]>,
    key_sources: Arc<[SessionAffinityKeySource]>,
    value_regex: Option<Regex>,
    ttl: Duration,
}

impl SessionAffinityRule {
    #[must_use]
    pub fn new(
        name: Arc<str>,
        fingerprint: [u8; 32],
        api_formats: Arc<[ApiFormat]>,
        model_regex: Arc<[Regex]>,
        key_sources: Arc<[SessionAffinityKeySource]>,
        value_regex: Option<Regex>,
        ttl: Duration,
    ) -> Self {
        Self {
            name,
            fingerprint,
            api_formats,
            model_regex,
            key_sources,
            value_regex,
            ttl,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    #[must_use]
    pub fn key_sources(&self) -> &[SessionAffinityKeySource] {
        &self.key_sources
    }

    #[must_use]
    pub const fn ttl(&self) -> Duration {
        self.ttl
    }

    #[must_use]
    pub fn matches_request(&self, api_format: ApiFormat, model: &str) -> bool {
        self.api_formats.contains(&api_format)
            && (self.model_regex.is_empty()
                || self
                    .model_regex
                    .iter()
                    .any(|pattern| pattern.is_match(model)))
    }

    #[must_use]
    pub fn matches_value(&self, value: &str) -> bool {
        self.value_regex
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(value))
    }
}

/// Immutable global policy for process-local, successful-channel session
/// affinity.
#[derive(Clone, Debug)]
pub struct SessionAffinitySettings {
    enabled: bool,
    max_entries: usize,
    default_ttl: Duration,
    rules: Arc<[SessionAffinityRule]>,
}

impl SessionAffinitySettings {
    #[must_use]
    pub fn new(
        enabled: bool,
        max_entries: usize,
        default_ttl: Duration,
        rules: Arc<[SessionAffinityRule]>,
    ) -> Self {
        Self {
            enabled,
            max_entries,
            default_ttl,
            rules,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }

    #[must_use]
    pub const fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    #[must_use]
    pub fn rules(&self) -> &[SessionAffinityRule] {
        &self.rules
    }
}

impl Default for SessionAffinitySettings {
    fn default() -> Self {
        Self::new(false, 100_000, Duration::from_secs(3_600), Arc::from([]))
    }
}

/// Immutable process-wide Responses WebSocket admission and idle-pool policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponsesWebSocketSettings {
    enabled: bool,
    max_idle_connections: usize,
    idle_timeout: Duration,
    max_connection_age: Duration,
}

impl ResponsesWebSocketSettings {
    #[must_use]
    pub const fn new(
        enabled: bool,
        max_idle_connections: usize,
        idle_timeout: Duration,
        max_connection_age: Duration,
    ) -> Self {
        Self {
            enabled,
            max_idle_connections,
            idle_timeout,
            max_connection_age,
        }
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn max_idle_connections(self) -> usize {
        self.max_idle_connections
    }

    #[must_use]
    pub const fn idle_timeout(self) -> Duration {
        self.idle_timeout
    }

    #[must_use]
    pub const fn max_connection_age(self) -> Duration {
        self.max_connection_age
    }
}

impl Default for ResponsesWebSocketSettings {
    fn default() -> Self {
        Self::new(
            false,
            128,
            Duration::from_secs(5 * 60),
            Duration::from_secs(55 * 60),
        )
    }
}

/// Immutable process-wide MCP transport policy.
///
/// The `mcp-server` feature controls whether the transport implementation is
/// linked into the binary. These values are database-backed runtime settings
/// and may be changed through the Console without restarting the process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpTransportSettings {
    enabled: bool,
    public_base_url: Option<Arc<str>>,
    allowed_hosts: Arc<[String]>,
    allowed_origins: Arc<[String]>,
    allow_legacy_2025_11_25: bool,
    request_body_bytes: usize,
    image_request_body_bytes: usize,
    search_result_bytes: usize,
    image_result_bytes: usize,
}

impl McpTransportSettings {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        enabled: bool,
        public_base_url: Option<Arc<str>>,
        allowed_hosts: Arc<[String]>,
        allowed_origins: Arc<[String]>,
        allow_legacy_2025_11_25: bool,
        request_body_bytes: usize,
        image_request_body_bytes: usize,
        search_result_bytes: usize,
        image_result_bytes: usize,
    ) -> Self {
        Self {
            enabled,
            public_base_url,
            allowed_hosts,
            allowed_origins,
            allow_legacy_2025_11_25,
            request_body_bytes,
            image_request_body_bytes,
            search_result_bytes,
            image_result_bytes,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn public_base_url(&self) -> Option<&str> {
        self.public_base_url.as_deref()
    }

    #[must_use]
    pub fn allowed_hosts(&self) -> &[String] {
        &self.allowed_hosts
    }

    #[must_use]
    pub fn allowed_origins(&self) -> &[String] {
        &self.allowed_origins
    }

    #[must_use]
    pub const fn allow_legacy_2025_11_25(&self) -> bool {
        self.allow_legacy_2025_11_25
    }

    #[must_use]
    pub const fn request_body_bytes(&self) -> usize {
        self.request_body_bytes
    }

    #[must_use]
    pub const fn image_request_body_bytes(&self) -> usize {
        self.image_request_body_bytes
    }

    #[must_use]
    pub const fn search_result_bytes(&self) -> usize {
        self.search_result_bytes
    }

    #[must_use]
    pub const fn image_result_bytes(&self) -> usize {
        self.image_result_bytes
    }
}

impl Default for McpTransportSettings {
    fn default() -> Self {
        Self::new(
            false,
            None,
            Arc::from([]),
            Arc::from([]),
            false,
            DEFAULT_MCP_REQUEST_BODY_BYTES,
            DEFAULT_MCP_IMAGE_REQUEST_BODY_BYTES,
            DEFAULT_MCP_SEARCH_RESULT_BYTES,
            DEFAULT_MCP_IMAGE_RESULT_BYTES,
        )
    }
}

/// Immutable connector-owned identity used by Codex backend requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexOutboundIdentity {
    originator: Arc<str>,
    client_version: Arc<str>,
    user_agent: Arc<str>,
}

impl CodexOutboundIdentity {
    #[must_use]
    pub fn new(originator: Arc<str>, client_version: Arc<str>, user_agent: Arc<str>) -> Self {
        Self {
            originator,
            client_version,
            user_agent,
        }
    }

    #[must_use]
    pub fn originator(&self) -> &str {
        &self.originator
    }

    #[must_use]
    pub fn client_version(&self) -> &str {
        &self.client_version
    }

    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
}

impl Default for CodexOutboundIdentity {
    fn default() -> Self {
        Self::new(
            Arc::from(DEFAULT_CODEX_ORIGINATOR),
            Arc::from(DEFAULT_CODEX_CLIENT_VERSION),
            Arc::from(DEFAULT_CODEX_USER_AGENT),
        )
    }
}

/// Immutable privacy-preserving metadata and outbound identity projected into
/// Codex Connect requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexRequestMetadataSettings {
    workspace_path: Arc<str>,
    git_remote_url: Arc<str>,
    outbound_identity: CodexOutboundIdentity,
}

impl CodexRequestMetadataSettings {
    #[must_use]
    pub fn new(
        workspace_path: Arc<str>,
        git_remote_url: Arc<str>,
        outbound_identity: CodexOutboundIdentity,
    ) -> Self {
        Self {
            workspace_path,
            git_remote_url,
            outbound_identity,
        }
    }

    #[must_use]
    pub fn workspace_path(&self) -> &str {
        &self.workspace_path
    }

    #[must_use]
    pub fn git_remote_url(&self) -> &str {
        &self.git_remote_url
    }

    #[must_use]
    pub const fn outbound_identity(&self) -> &CodexOutboundIdentity {
        &self.outbound_identity
    }
}

impl Default for CodexRequestMetadataSettings {
    fn default() -> Self {
        Self::new(
            Arc::from(DEFAULT_CODEX_WORKSPACE_PATH),
            Arc::from(DEFAULT_CODEX_GIT_REMOTE_URL),
            CodexOutboundIdentity::default(),
        )
    }
}

/// Immutable global runtime policy published with each configuration snapshot.
#[derive(Clone, Debug)]
pub struct SystemRuntimeSettings {
    upstream_timeouts: UpstreamTimeoutDefaults,
    request_retry: RequestRetrySettings,
    passive_health: PassiveHealthSettings,
    automatic_disable: AutomaticDisableSettings,
    scheduled_testing: ScheduledTestingSettings,
    session_affinity: SessionAffinitySettings,
    websocket: ResponsesWebSocketSettings,
    mcp: McpTransportSettings,
    codex: CodexRequestMetadataSettings,
}

impl SystemRuntimeSettings {
    #[must_use]
    pub fn new(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            request_retry: RequestRetrySettings::default(),
            passive_health,
            automatic_disable: AutomaticDisableSettings::default(),
            scheduled_testing: ScheduledTestingSettings::default(),
            session_affinity: SessionAffinitySettings::default(),
            websocket: ResponsesWebSocketSettings::default(),
            mcp: McpTransportSettings::default(),
            codex: CodexRequestMetadataSettings::default(),
        }
    }

    #[must_use]
    pub fn new_with_websocket(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
        websocket: ResponsesWebSocketSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            request_retry: RequestRetrySettings::default(),
            passive_health,
            automatic_disable: AutomaticDisableSettings::default(),
            scheduled_testing: ScheduledTestingSettings::default(),
            session_affinity: SessionAffinitySettings::default(),
            websocket,
            mcp: McpTransportSettings::default(),
            codex: CodexRequestMetadataSettings::default(),
        }
    }

    #[must_use]
    pub fn new_with_channel_automation(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
        automatic_disable: AutomaticDisableSettings,
        scheduled_testing: ScheduledTestingSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            request_retry: RequestRetrySettings::default(),
            passive_health,
            automatic_disable,
            scheduled_testing,
            session_affinity: SessionAffinitySettings::default(),
            websocket: ResponsesWebSocketSettings::default(),
            mcp: McpTransportSettings::default(),
            codex: CodexRequestMetadataSettings::default(),
        }
    }

    #[must_use]
    pub fn new_with_all(
        upstream_timeouts: UpstreamTimeoutDefaults,
        request_retry: RequestRetrySettings,
        passive_health: PassiveHealthSettings,
        automatic_disable: AutomaticDisableSettings,
        scheduled_testing: ScheduledTestingSettings,
        session_affinity: SessionAffinitySettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            request_retry,
            passive_health,
            automatic_disable,
            scheduled_testing,
            session_affinity,
            websocket: ResponsesWebSocketSettings::default(),
            mcp: McpTransportSettings::default(),
            codex: CodexRequestMetadataSettings::default(),
        }
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_all_and_websocket(
        upstream_timeouts: UpstreamTimeoutDefaults,
        request_retry: RequestRetrySettings,
        passive_health: PassiveHealthSettings,
        automatic_disable: AutomaticDisableSettings,
        scheduled_testing: ScheduledTestingSettings,
        session_affinity: SessionAffinitySettings,
        websocket: ResponsesWebSocketSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            request_retry,
            passive_health,
            automatic_disable,
            scheduled_testing,
            session_affinity,
            websocket,
            mcp: McpTransportSettings::default(),
            codex: CodexRequestMetadataSettings::default(),
        }
    }

    #[must_use]
    pub fn with_mcp(mut self, mcp: McpTransportSettings) -> Self {
        self.mcp = mcp;
        self
    }

    #[must_use]
    pub fn with_codex(mut self, codex: CodexRequestMetadataSettings) -> Self {
        self.codex = codex;
        self
    }

    #[must_use]
    pub const fn upstream_timeouts(&self) -> UpstreamTimeoutDefaults {
        self.upstream_timeouts
    }

    #[must_use]
    pub const fn request_retry(&self) -> RequestRetrySettings {
        self.request_retry
    }

    #[must_use]
    pub const fn passive_health(&self) -> PassiveHealthSettings {
        self.passive_health
    }

    #[must_use]
    pub fn automatic_disable(&self) -> &AutomaticDisableSettings {
        &self.automatic_disable
    }

    #[must_use]
    pub fn scheduled_testing(&self) -> &ScheduledTestingSettings {
        &self.scheduled_testing
    }

    #[must_use]
    pub fn session_affinity(&self) -> &SessionAffinitySettings {
        &self.session_affinity
    }

    #[must_use]
    pub const fn websocket(&self) -> ResponsesWebSocketSettings {
        self.websocket
    }

    #[must_use]
    pub const fn mcp(&self) -> &McpTransportSettings {
        &self.mcp
    }

    #[must_use]
    pub const fn codex(&self) -> &CodexRequestMetadataSettings {
        &self.codex
    }
}

impl Default for SystemRuntimeSettings {
    fn default() -> Self {
        Self::new(
            UpstreamTimeoutDefaults::default(),
            PassiveHealthSettings::default(),
        )
    }
}
