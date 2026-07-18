//! Application use cases: proxying, model listing, and configuration management.

mod control_plane;
mod model_sync;
mod proxy;
mod request_log;

pub use control_plane::{ControlPlaneCoordinator, ControlPlaneError, ModelSyncResult};
pub use model_sync::{
    ModelSyncError, ModelSyncPreviewRequest, ModelSyncRequest, ModelSyncResponse, ModelSyncService,
};
pub use proxy::{ModelsResponse, ProxyError, ProxyService};
pub use request_log::{
    NoopRequestLogSink, QueueRequestLogSink, RecordingRequestLogSink, RequestLogSink,
};
