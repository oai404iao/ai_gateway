//! Application use cases: proxying, model listing, Console authentication, and configuration management.

mod auth;
mod billing;
mod channel_automation;
mod channel_models;
mod codex;
mod connector;
mod control_plane;
mod model_sync;
mod proxy;
mod proxy_test;
mod request_body;
mod request_log;
mod system_metrics;
mod usage;

pub use auth::{
    AuthError, ConsoleAuthService, ConsoleUser, IssuedInvitation, IssuedRegistrationInvitationCode,
    IssuedSession, IssuedTemporaryPassword, RegistrationInvitationCodeCreateInput,
    RegistrationInvitationCodeMutation, RegistrationInvitationCodeUpdateInput,
    SelfRegistrationInput, hash_console_password,
};
pub(crate) use billing::{
    request_billing, request_billing_multiplier, request_billing_multiplier_for_value,
};
pub use channel_automation::{
    AutomaticDisableService, AutomaticDisableWorker, ErrorKeywordMatcher,
};
pub use channel_models::{
    ChannelModelDiscoveryError, ChannelModelDiscoveryInput, ChannelModelDiscoveryResponse,
    ChannelModelDiscoveryService,
};
pub use codex::{
    CODEX_ORIGINATOR, CodexConnectorError, CodexConnectorService, CodexCredentialRuntime,
    CodexCredentialUnavailable, CodexOauthCompleteInput, CodexOauthStartResponse,
    CodexQuotaResetResponse, CompiledCodexCredential, codex_user_agent,
};
pub use connector::UpstreamConnectorRegistry;
pub(crate) use connector::{ConnectorAttemptError, ConnectorUnavailable};
pub use control_plane::{
    ChannelBatchUpdateResult, CodexCredentialBatchResult, ControlPlaneCoordinator,
    ControlPlaneError, ModelSyncResult, UserBatchUpdateResult,
};
pub use model_sync::{
    ModelImportRequest, ModelSyncError, ModelSyncPreview, ModelSyncPreviewRequest,
    ModelSyncResponse, ModelSyncService,
};
pub use proxy::{ModelsResponse, ProxyError, ProxyService};
pub use proxy_test::{ProxyTestError, ProxyTestInput, ProxyTestResponse, ProxyTestService};
pub use request_body::ProxyRequestBodyLimits;
#[cfg(all(test, feature = "mcp-server"))]
pub(crate) use request_body::{PreparedRequestBody, ReplayableRequestBody};
pub use request_log::{
    DurableRequestLogSink, NoopRequestLogSink, QueueRequestLogSink, RecordingRequestLogSink,
    RequestLogPipelineMonitor, RequestLogSink,
};
pub use system_metrics::{SystemLoadReport, SystemMetricsService};
pub(crate) use usage::{ResponseErrorDetails, ResponseUsage, UsageCollector};
