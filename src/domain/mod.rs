//! Entities, value objects, invariants, and ports.

mod api_format;
mod api_key;
mod compiled_routing;
mod console_auth;
mod request_log;
mod system_settings;

pub use api_format::ApiFormat;
pub use api_key::ApiKeyHash;
pub use compiled_routing::{
    ApiKeyPermission, ChannelTimeoutPolicy, CompiledApiKey, CompiledChannel, CompiledChannelGroup,
    CompiledChannelUpstreamPolicy, CompiledConfigTemplate, CompiledModelRule, CompiledProxy,
    CompiledRouteTier, CompiledRuntimeConfig, CompiledUnavailableRouteCandidate,
    ModelPriceSnapshot, ModelRouteKey, NoProxyHost, NoProxyHostError,
    OutboundNetworkPolicyFingerprint, SelectionStrategy, UpstreamAuth,
};
pub use console_auth::{ConsolePrincipal, UserRole};
pub use request_log::{
    RequestBilling, RequestLogEvent, RequestLogOutcome, RequestLogSource, RequestPriceSnapshot,
    RequestUsage,
};
pub use system_settings::{
    AutomaticDisableSettings, AutomaticDisableTrigger, PassiveHealthSettings, ScheduledTestingMode,
    ScheduledTestingSettings, SessionAffinityKeySource, SessionAffinityRule,
    SessionAffinitySettings, SystemRuntimeSettings, UpstreamTimeoutDefaults,
};
