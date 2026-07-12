//! Application use cases: proxying, model listing, and configuration management.

mod proxy;
mod request_log;

pub use proxy::{ModelsResponse, ProxyError, ProxyService};
pub use request_log::{
    NoopRequestLogSink, QueueRequestLogSink, RecordingRequestLogSink, RequestLogSink,
};
