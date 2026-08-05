//! Entities, value objects, invariants, and ports.

mod api_format;
mod api_key;
mod api_operation;
mod billing;
mod compiled_routing;
mod connector;
mod console_auth;
mod request_log;
mod system_settings;

pub use api_format::ApiFormat;
pub use api_key::ApiKeyHash;
pub use api_operation::ApiOperation;
pub use billing::{
    AdvancedBilling, AdvancedBillingError, CompiledAdvancedBilling, LongContextTier,
    RequestBillingMultiplier,
};
pub use compiled_routing::{
    ApiKeyPermission, AuthorizationProfile, ChannelTimeoutPolicy, CompiledApiKey,
    CompiledCandidate, CompiledChannel, CompiledChannelGroup, CompiledChannelUpstreamPolicy,
    CompiledConfigTemplate, CompiledModelRule, CompiledProxy, CompiledRouteTier,
    CompiledRuntimeConfig, CompiledScheduledTestModel, CompiledUnavailableRouteCandidate,
    ModelPriceSnapshot, ModelRouteKey, NoProxyHost, NoProxyHostError,
    OutboundNetworkPolicyFingerprint, SelectionStrategy, UpstreamAuth,
};
pub use connector::ConnectorKind;
pub use console_auth::{ConsolePrincipal, ConsoleSessionPurpose, UserRole};
pub use request_log::{
    RequestBilling, RequestLogEvent, RequestLogOutcome, RequestLogSource, RequestPriceSnapshot,
    RequestProtocol, RequestUsage,
};
pub use system_settings::{
    AutomaticDisableSettings, AutomaticDisableTrigger,
    DEFAULT_IMAGES_RESPONSE_HEADER_TIMEOUT_SECONDS,
    DEFAULT_STANDALONE_WEB_SEARCH_RESPONSE_HEADER_TIMEOUT_SECONDS, MAX_REQUEST_RETRIES,
    PassiveHealthSettings, RequestRetrySettings, ResponsesWebSocketSettings, ScheduledTestingMode,
    ScheduledTestingSettings, SessionAffinityKeySource, SessionAffinityRule,
    SessionAffinitySettings, SystemRuntimeSettings, UpstreamTimeoutDefaults,
};
