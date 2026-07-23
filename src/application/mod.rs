//! Application use cases: proxying, model listing, Console authentication, and configuration management.

mod auth;
mod channel_automation;
mod control_plane;
mod model_sync;
mod proxy;
mod request_log;
mod system_metrics;
mod usage;

pub use auth::{
    AuthError, ConsoleAuthService, ConsoleUser, IssuedInvitation, IssuedSession,
    hash_console_password,
};
pub use channel_automation::{
    AutomaticDisableService, AutomaticDisableWorker, ErrorKeywordMatcher,
};
pub use control_plane::{
    ChannelBatchUpdateResult, ControlPlaneCoordinator, ControlPlaneError, ModelSyncResult,
};
pub use model_sync::{
    ModelImportRequest, ModelSyncError, ModelSyncPreview, ModelSyncPreviewRequest,
    ModelSyncResponse, ModelSyncService,
};
pub use proxy::{ModelsResponse, ProxyError, ProxyService};
pub use request_log::{
    DurableRequestLogSink, NoopRequestLogSink, QueueRequestLogSink, RecordingRequestLogSink,
    RequestLogPipelineMonitor, RequestLogSink,
};
pub use system_metrics::{SystemLoadReport, SystemMetricsService};
