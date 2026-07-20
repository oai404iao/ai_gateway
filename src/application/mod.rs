//! Application use cases: proxying, model listing, Console authentication, and configuration management.

mod auth;
mod control_plane;
mod model_sync;
mod proxy;
mod request_log;
mod usage;

pub use auth::{
    AuthError, ConsoleAuthService, ConsoleUser, IssuedInvitation, IssuedSession,
    hash_console_password,
};
pub use control_plane::{ControlPlaneCoordinator, ControlPlaneError, ModelSyncResult};
pub use model_sync::{
    ModelImportRequest, ModelSyncError, ModelSyncPreview, ModelSyncPreviewRequest,
    ModelSyncResponse, ModelSyncService,
};
pub use proxy::{ModelsResponse, ProxyError, ProxyService};
pub use request_log::{
    NoopRequestLogSink, QueueRequestLogSink, RecordingRequestLogSink, RequestLogSink,
};
